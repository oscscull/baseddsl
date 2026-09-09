//! Aggregate queries: GROUP BY, aggregate SELECT list, and HAVING.

use super::*;

/// Whether a shape body carries a top-level aggregate field (`= count()` / `= sum(…)`),
/// making it an aggregate shape — projected over groups, not rows.
pub(crate) fn shape_has_agg(body: &[ShapeField]) -> bool {
    body.iter().any(|f| {
        matches!(
            f,
            ShapeField::Rename {
                value: ShapeValue::Agg(_),
                ..
            }
        )
    })
}

/// The `GROUP BY` / `HAVING` SELECT for an aggregate query. The flat aggregate shape (sema
/// guarantees flatness) projects group columns + aggregate expressions; the query's `group
/// by` lists the grouping columns, `having` filters groups, `order` names projected columns.
/// No pagination/keyset — an aggregate query is not paginated.
pub(crate) fn lower_agg_query<'a>(
    q: &'a Query,
    root: &'a RModel,
    mut sel: Select<'a>,
    body: &'a [ShapeField],
    dialect: Dialect,
) -> LoweredQuery {
    // Projection: each field is a group column (its `table.col`) or an aggregate
    // expression, aliased to the shape's output name. Two expressions per aggregate: the
    // *projection* form (decode-cast so the wire type is right) in the SELECT list, and the
    // *comparison* form (the plain numeric aggregate) supplied to `HAVING`/`ORDER BY` — an
    // alias isn't portable in `HAVING`, and the SELECT's decimal-to-text cast would break
    // an ordered comparison.
    let mut cols: Vec<String> = Vec::new();
    let mut expr_map: HashMap<String, String> = HashMap::new();
    // out-name → the model column path a group (non-aggregate) field projects, so
    // `having` can render an enum-typed group column's RHS variant as its wire literal.
    let mut group_col_paths: HashMap<String, Path> = HashMap::new();
    let root_alias = sel.root_alias.clone();
    for f in body {
        let (out, proj, cmp) = match f {
            ShapeField::Bare(id) => {
                let (a, c) = sel.resolve_from(&single(&id.node), &root_alias, "", root);
                let q = sel.qcol(&a, &c);
                group_col_paths.insert(id.node.clone(), single(&id.node));
                (id.node.clone(), q.clone(), q)
            }
            ShapeField::Rename { out, value } => match value {
                ShapeValue::Path(p) => {
                    let (a, c) = sel.resolve_from(p, &root_alias, "", root);
                    let q = sel.qcol(&a, &c);
                    group_col_paths.insert(out.node.clone(), p.clone());
                    (out.node.clone(), q.clone(), q)
                }
                ShapeValue::Agg(agg) => {
                    let proj = agg_sql(&mut sel, root, &root_alias, "", agg, dialect, true);
                    let cmp = agg_sql(&mut sel, root, &root_alias, "", agg, dialect, false);
                    (out.node.clone(), proj, cmp)
                }
                ShapeValue::Raw(raw) => {
                    let e = format!("({})", render_raw(dialect, raw, &root_alias, &root.table));
                    (out.node.clone(), e.clone(), e)
                }
                // A computed field never co-occurs with an aggregate (sema `E0324`); lower
                // it defensively so the branch is total.
                ShapeValue::Computed(expr) => {
                    let e = sel.shape_expr(expr, &root_alias, "", root);
                    (out.node.clone(), e.clone(), e)
                }
            },
            // Aggregate shapes are flat (sema `E0245`); a stray nest/flatten is ignored.
            ShapeField::Nest { .. } | ShapeField::NestRef { .. } | ShapeField::Flatten { .. } => {
                continue
            }
            ShapeField::Spread { .. } => unreachable!("spreads expanded before codegen"),
        };
        cols.push(format!("  {} AS {}", proj, sel.q(&out)));
        expr_map.insert(out, cmp);
    }
    let projection = cols.join(",\n");

    // Row filter (before grouping): the query's own `where`, then soft-delete + `@scope`.
    let mut wheres: Vec<String> = Vec::new();
    collect_filter(&mut sel, q, root, &mut wheres);
    if let Some(sd) = &root.soft_delete {
        wheres.push(soft_pred(dialect, &sel.root_alias, root, sd));
    }
    if let Some(scope) = sel.scope_where(&sel.root_alias, root) {
        wheres.push(scope);
    }

    // `group by` columns, `having`, and `order` (all naming projected columns).
    let (group_paths, having_pred, order_terms) = agg_clause_parts(q);
    let group_by: Vec<String> = group_paths
        .iter()
        .map(|p| {
            let (a, c) = sel.resolve(p, root);
            sel.qcol(&a, &c)
        })
        .collect();
    let having = having_pred.map(|hp| sel.having(hp, root, &expr_map, &group_col_paths));
    let order: Vec<String> = order_terms
        .iter()
        .filter_map(|t| {
            let seg = t.path.segments.first()?;
            let expr = expr_map
                .get(&seg.node)
                .cloned()
                .unwrap_or_else(|| sel.q(&seg.node));
            // The agg sort expr may be a group column or an aggregate; nullability isn't cleanly
            // available here, so pass `true` — the NULLS clause / `IS NULL` prefix is a harmless
            // no-op on a non-null operand.
            Some(order_by_term(&expr, t.dir, true, t.nulls, sel.dialect))
        })
        .collect();

    // Assemble — joins were accumulated by the resolves above.
    let mut sql = format!("SELECT\n{}\nFROM {}", projection, sel.qt(root));
    push_joins(&mut sql, sel.dialect, &sel.joins);
    if !wheres.is_empty() {
        sql.push_str(&format!("\nWHERE {}", wheres.join(" AND ")));
    }
    if !group_by.is_empty() {
        sql.push_str(&format!("\nGROUP BY {}", group_by.join(", ")));
    }
    if let Some(h) = having {
        sql.push_str(&format!("\nHAVING {h}"));
    }
    if !order.is_empty() {
        sql.push_str(&format!("\nORDER BY {}", order.join(", ")));
    }
    sql.push_str(";\n");

    LoweredQuery {
        name: q.name.node.clone(),
        sql,
        count_sql: None,
        keyset: None,
        // An aggregate shape is flat scalars (`E0245` forbids a nest); only a bare `json`
        // group key would need normalizing, so walk the body with no shape registry.
        json_paths: {
            let mut v = Vec::new();
            walk_shape_json(sel.schema, &[], body, root, "", &mut v);
            v
        },
    }
}

/// The `group by` paths, `having` predicate, and `order` terms of an aggregate query's
/// body (inline or block).
fn agg_clause_parts(q: &Query) -> (Vec<&Path>, Option<&Predicate>, Vec<&SortTerm>) {
    let clauses: &[Clause] = match &q.body {
        QueryBody::Inline(cs) => cs,
        QueryBody::Block(s) => &s.clauses,
        _ => &[],
    };
    let mut groups = Vec::new();
    let mut having = None;
    let mut order = Vec::new();
    for c in clauses {
        match c {
            Clause::GroupBy(cols) => groups.extend(cols.iter()),
            Clause::Having(p) => having = Some(p),
            Clause::Order(terms) => order.extend(terms.iter()),
            _ => {}
        }
    }
    (groups, having, order)
}

/// One aggregate expression. In `projection` position it is dialect-cast so the driver
/// decodes it to the wire type the client expects: `count` → an integer; `sum` keeps the
/// column's numeric family (cast back where a dialect widens it — Postgres/MariaDB widen
/// `SUM(int)`, SQLite renders a decimal sum as text for the exact-string wire form); `avg`
/// → a double; `min`/`max` keep the column's own type. In *comparison* position
/// (`HAVING`/`ORDER BY`) the decimal-to-text cast is dropped so an ordered comparison stays
/// numeric.
pub(crate) fn agg_sql(
    sel: &mut Select,
    model: &RModel,
    alias: &str,
    prefix: &str,
    agg: &AggCall,
    dialect: Dialect,
    projection: bool,
) -> String {
    let func = agg.func.node.as_str();
    if func == "count" {
        return "COUNT(*)".to_string();
    }
    let Some(path) = &agg.arg else {
        return "COUNT(*)".to_string(); // sema guarantees an arg for non-count
    };
    let (a, c) = sel.resolve_from(path, alias, prefix, model);
    let cref = sel.qcol(&a, &c);
    let prim = path_primitive(sel.schema, model, path);
    match func {
        "avg" => format!("CAST(AVG({cref}) AS {})", double_cast_type(dialect)),
        "sum" => match prim {
            Primitive::Int => match int_cast_type(dialect) {
                Some(t) => format!("CAST(SUM({cref}) AS {t})"),
                None => format!("SUM({cref})"),
            },
            Primitive::Decimal { .. } if dialect == Dialect::Sqlite && projection => {
                format!("CAST(SUM({cref}) AS TEXT)")
            }
            _ => format!("SUM({cref})"),
        },
        "min" => format!("MIN({cref})"),
        "max" => format!("MAX({cref})"),
        _ => "COUNT(*)".to_string(),
    }
}

/// The `CAST(… AS <int>)` target that coerces a widened `SUM(int)` back to an integer, or
/// `None` where the dialect keeps it integral (SQLite). MariaDB/Postgres widen `SUM` of a
/// `BIGINT` to decimal/numeric, which would decode as a string; the cast keeps it a number.
fn int_cast_type(dialect: Dialect) -> Option<&'static str> {
    match dialect {
        Dialect::MariaDb | Dialect::MySql => Some("SIGNED"),
        Dialect::Postgres => Some("BIGINT"),
        Dialect::Sqlite => None,
    }
}

/// The dialect's double type, the `CAST` target that makes `AVG` decode as a float number
/// on every dialect (Postgres `AVG` of an int/numeric is otherwise a numeric string).
fn double_cast_type(dialect: Dialect) -> &'static str {
    match dialect {
        Dialect::MariaDb | Dialect::MySql => "DOUBLE",
        Dialect::Postgres => "DOUBLE PRECISION",
        Dialect::Sqlite => "REAL",
    }
}

impl<'a> Select<'a> {
    /// Lower a `having` predicate. Its left operands name the aggregate shape's projected
    /// columns, so each single-segment path is resolved through `map` (out-name → the SQL
    /// expression it aliases) — an aggregate alias or a group column. `HAVING` can't
    /// portably reference the SELECT alias (Postgres forbids it), so the expression is
    /// inlined. Right-hand values render exactly as in `where`.
    pub(crate) fn having(
        &mut self,
        p: &Predicate,
        model: &RModel,
        map: &HashMap<String, String>,
        group_paths: &HashMap<String, Path>,
    ) -> String {
        match p {
            Predicate::And(a, b) => {
                format!(
                    "({} AND {})",
                    self.having(a, model, map, group_paths),
                    self.having(b, model, map, group_paths)
                )
            }
            Predicate::Or(a, b) => {
                format!(
                    "({} OR {})",
                    self.having(a, model, map, group_paths),
                    self.having(b, model, map, group_paths)
                )
            }
            Predicate::Not(inner) => {
                format!("NOT ({})", self.having(inner, model, map, group_paths))
            }
            Predicate::Cmp { path, op, value } => {
                let lhs = self.having_lhs(path, model, map);
                if matches!(value, Value::Lit(Literal::Null)) && matches!(op, Op::Eq | Op::Ne) {
                    // A null test on a group column / aggregate: IS [NOT] NULL, not `= NULL`.
                    let nullness = if *op == Op::Eq {
                        "IS NULL"
                    } else {
                        "IS NOT NULL"
                    };
                    format!("{lhs} {nullness}")
                } else {
                    let rhs = self
                        .having_variant_lit(path, model, value, group_paths)
                        .unwrap_or_else(|| self.value(value, model));
                    match op {
                        Op::In => format!("{lhs} IN ({rhs})"),
                        Op::Has => match self.dialect {
                            Dialect::Postgres => format!("{lhs} @> {rhs}"),
                            _ => format!("{rhs} MEMBER OF({lhs})"),
                        },
                        _ => format!("{lhs} {} {rhs}", sql_op(*op)),
                    }
                }
            }
            Predicate::InList { path, values } => {
                let lhs = self.having_lhs(path, model, map);
                let items: Vec<String> = values
                    .iter()
                    .map(|v| {
                        self.having_variant_lit(path, model, v, group_paths)
                            .unwrap_or_else(|| self.value(v, model))
                    })
                    .collect();
                format!("{lhs} IN ({})", items.join(", "))
            }
            Predicate::Bare(path) => {
                let lhs = self.having_lhs(path, model, map);
                format!("{lhs} = {}", self.dialect.bool_lit(true))
            }
            Predicate::Raw(raw) => format!(
                "({})",
                render_raw(self.dialect, raw, &self.root_alias, &model.table)
            ),
            // A named-filter call has no aggregate meaning in `having`; sema leaves it
            // unchecked, so lower it to a harmless truth.
            Predicate::FilterCall { .. } => "TRUE".to_string(),
        }
    }

    /// The left-operand SQL for a `having` comparison: the projected column's aliased
    /// expression from `map`, falling back to a column resolve (sema keeps it in `map`).
    fn having_lhs(&mut self, path: &Path, model: &RModel, map: &HashMap<String, String>) -> String {
        if let Some(seg) = path.segments.first() {
            if path.segments.len() == 1 {
                if let Some(expr) = map.get(&seg.node) {
                    return expr.clone();
                }
            }
        }
        let (a, c) = self.resolve(path, model);
        self.qcol(&a, &c)
    }

    /// A `having` RHS that is a bare variant of an enum-typed group column, as its wire
    /// literal — the `where`-side enum rendering, applied through the projected out-name's
    /// underlying column path (so a renamed group column resolves too). `None` (fall back to
    /// ordinary value lowering) unless the left operand names an enum group column.
    fn having_variant_lit(
        &self,
        path: &Path,
        model: &RModel,
        value: &Value,
        group_paths: &HashMap<String, Path>,
    ) -> Option<String> {
        let out = path.segments.first()?;
        let col_path = group_paths.get(&out.node)?;
        self.enum_variant_lit(model, col_path, value)
    }
}
