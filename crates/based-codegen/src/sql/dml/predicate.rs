//! WHERE lowering: filters, present-guards, null-safe context reads, named-filter inlining, soft-delete.

use super::*;

/// Dialect null-safe equality (`negated` → inequality). Unlike plain `=`, a NULL on either
/// side is a real operand: `NULL` matches `NULL`, and `col <=> NULL` is `col IS NULL`. Used
/// where an absent value binds to SQL NULL yet must still match the rows that are themselves
/// unset — an optional `$ctx.field?` read whose field is absent (auth.md Handle 1).
fn nullsafe_cmp(dialect: Dialect, lhs: &str, rhs: &str, negated: bool) -> String {
    let eq = match dialect {
        Dialect::Sqlite => format!("{lhs} IS {rhs}"),
        Dialect::MariaDb | Dialect::MySql => format!("{lhs} <=> {rhs}"),
        Dialect::Postgres => format!("{lhs} IS NOT DISTINCT FROM {rhs}"),
    };
    if negated {
        format!("NOT ({eq})")
    } else {
        eq
    }
}

/// A `$ctx.<field>?` optional context read (auth.md Handle 1) — an absent field binds SQL
/// NULL. Distinct from an optional `?` filter param (a marker on the signature).
fn is_optional_ctx_read(v: &Value) -> bool {
    matches!(v, Value::Param(pr) if pr.optional && pr.name.node == "ctx" && pr.path.len() == 1)
}

// ---------- filters (the query's own predicate) ---------------------------

/// Append the query's filter conditions. Bare/inline queries map each param to a
/// same-name equality (or its per-param binding); block/inline queries also carry
/// explicit `where` clauses referencing params via `$`.
pub(crate) fn collect_filter(sel: &mut Select, q: &Query, root: &RModel, out: &mut Vec<String>) {
    sel.optional_params = q
        .params
        .iter()
        .filter(|p| p.optional)
        .map(|p| p.name.node.clone())
        .collect();
    let is_block = matches!(q.body, QueryBody::Block(_) | QueryBody::Raw(_));
    if !is_block {
        for p in &q.params {
            out.push(param_condition(sel, p, root));
        }
    }
    let clauses: &[Clause] = match &q.body {
        QueryBody::Inline(cs) => cs,
        QueryBody::Block(s) => &s.clauses,
        QueryBody::Bare | QueryBody::Raw(_) => &[],
    };
    for c in clauses {
        if let Clause::Where(pred) = c {
            out.push(sel.predicate(pred, root));
        }
    }
}

/// One bare/inline param -> a filter condition (per-param bindings). A `?` optional param's
/// predicate is wrapped in a present-guard so it drops when the arg is absent (queries.md) —
/// works for any operator, not just equality.
fn param_condition(sel: &mut Select, p: &Param, root: &RModel) -> String {
    let ph = format!(":{}", p.name.node);
    let raw = match &p.binding {
        // `user -> author`: equality on the named relation's FK column.
        Some(ParamBinding::Edge(edge)) => {
            let (alias, col) = sel.resolve(&single(&edge.node), root);
            format!("{} = {ph}", sel.qcol(&alias, &col))
        }
        // `since: timestamp > created_at`: explicit column + operator. The collection
        // ops mirror the predicate lowering — `in` takes a value list, `has` is JSON
        // containment (Postgres `col @> value`, MySQL-family `value MEMBER OF(col)`).
        Some(ParamBinding::ColOp { op, col }) => {
            let (alias, c) = sel.resolve(&single(&col.node), root);
            let lhs = sel.qcol(&alias, &c);
            match op {
                Op::In => format!("{lhs} IN ({ph})"),
                Op::Has => match sel.dialect {
                    Dialect::Postgres => format!("{lhs} @> {ph}"),
                    _ => format!("{ph} MEMBER OF({lhs})"),
                },
                _ => format!("{lhs} {} {ph}", sql_op(*op)),
            }
        }
        // same-name equality on the mapped column (a relation field maps to its FK).
        None => {
            let (alias, col) = sel.resolve(&single(&p.name.node), root);
            format!("{} = {ph}", sel.qcol(&alias, &col))
        }
    };
    present_guard(p, &raw)
}

/// Wrap a param's predicate in its optional-filter guard. A non-optional param's predicate is
/// used verbatim; a `?` optional param (queries.md) is guarded by a companion `:p__present`
/// flag the runtime binds — `0` when the arg is absent (the whole clause is a no-op, so the
/// filter drops), `1` when supplied. Operator-agnostic: an absent arg widens the leaf to TRUE,
/// so it composes correctly through `and`/`or` for `~`, ranges, `in`, `has`, or equality.
fn present_guard(p: &Param, predicate: &str) -> String {
    if !p.optional {
        return predicate.to_string();
    }
    let present = format!(":{}__present", p.name.node);
    format!("({present} = 0 OR {predicate})")
}

/// Substitute a filter's param bindings into its body. A filter param appears only
/// in value position (`= $c`, or an argument to a nested filter), so only `$name`
/// refs are rewritten; column paths are left to resolve against the call-site model.
fn subst_pred(p: &Predicate, binds: &HashMap<&str, &Value>) -> Predicate {
    match p {
        Predicate::And(a, b) => Predicate::And(
            Box::new(subst_pred(a, binds)),
            Box::new(subst_pred(b, binds)),
        ),
        Predicate::Or(a, b) => Predicate::Or(
            Box::new(subst_pred(a, binds)),
            Box::new(subst_pred(b, binds)),
        ),
        Predicate::Not(inner) => Predicate::Not(Box::new(subst_pred(inner, binds))),
        Predicate::Cmp { path, op, value } => Predicate::Cmp {
            path: path.clone(),
            op: *op,
            value: subst_value(value, binds),
        },
        Predicate::InList { path, values } => Predicate::InList {
            path: path.clone(),
            values: values.iter().map(|v| subst_value(v, binds)).collect(),
        },
        Predicate::Bare(path) => Predicate::Bare(path.clone()),
        Predicate::FilterCall { name, args } => Predicate::FilterCall {
            name: name.clone(),
            args: args.iter().map(|a| subst_value(a, binds)).collect(),
        },
        Predicate::Raw(raw) => Predicate::Raw(raw.clone()),
    }
}

/// Replace a bare `$name` value with its bound argument. A `$name.path` or an
/// unbound `$name` (e.g. `$ctx`) is left untouched; nested function args recurse.
fn subst_value(v: &Value, binds: &HashMap<&str, &Value>) -> Value {
    match v {
        Value::Param(pr) if pr.path.is_empty() => match binds.get(pr.name.node.as_str()) {
            Some(rep) => (*rep).clone(),
            None => v.clone(),
        },
        Value::Func(f) => Value::Func(FuncCall {
            name: f.name.clone(),
            args: f.args.iter().map(|a| subst_value(a, binds)).collect(),
        }),
        _ => v.clone(),
    }
}

// ---------- small helpers --------------------------------------------------

/// Soft-delete predicate for a table alias.
pub(crate) fn soft_pred(dialect: Dialect, alias: &str, model: &RModel, sd: &SoftDelete) -> String {
    let col = column_of(model, &sd.field);
    match sd.mode {
        SoftMode::Timestamp => format!("{} IS NULL", dialect.qcol(alias, &col)),
        SoftMode::Bool => format!(
            "{} = {}",
            dialect.qcol(alias, &col),
            dialect.bool_lit(false)
        ),
    }
}

impl<'a> Select<'a> {
    // ---------- predicate lowering (where / @scope) -----------------------

    /// Wrap a body predicate in its optional-filter present-guard when its RHS is a `?` param.
    /// An absent arg sets `:{name}__present` = 0, widening the leaf to TRUE so the filter drops
    /// and composes through `and`/`or` (queries.md). A non-optional RHS is returned verbatim.
    fn guard_optional(&self, value: &Value, pred: String) -> String {
        if let Value::Param(pr) = value {
            if pr.path.is_empty() && self.optional_params.contains(pr.name.node.as_str()) {
                return format!("(:{}__present = 0 OR {pred})", pr.name.node);
            }
        }
        pred
    }

    pub(crate) fn predicate(&mut self, p: &Predicate, model: &RModel) -> String {
        match p {
            Predicate::And(a, b) => {
                format!(
                    "({} AND {})",
                    self.predicate(a, model),
                    self.predicate(b, model)
                )
            }
            Predicate::Or(a, b) => {
                format!(
                    "({} OR {})",
                    self.predicate(a, model),
                    self.predicate(b, model)
                )
            }
            Predicate::Not(inner) => format!("NOT ({})", self.predicate(inner, model)),
            Predicate::Cmp { path, op, value } => {
                let (alias, col) = self.resolve(path, model);
                let lhs = self.qcol(&alias, &col);
                // Optional context read `$ctx.field?` (auth.md Handle 1): an absent field is
                // SQL NULL, so `=`/`!=` against it lower to null-safe (in)equality — absent
                // matches the rows whose own column is unset (`col IS NULL`) rather than
                // widening the filter. (A non-`=` operator keeps a plain comparison, where a
                // NULL bind simply matches nothing.) No present-guard: the null is the signal.
                if is_optional_ctx_read(value) && matches!(op, Op::Eq | Op::Ne) {
                    let rhs = self.value(value, model);
                    return nullsafe_cmp(self.dialect, &lhs, &rhs, *op == Op::Ne);
                }
                let pred = if matches!(value, Value::Lit(Literal::Null))
                    && matches!(op, Op::Eq | Op::Ne)
                {
                    // `col = null` / `col != null` are null tests, not value comparisons:
                    // SQL `col = NULL` is always NULL (never matches), so lower to IS [NOT] NULL.
                    let nullness = if *op == Op::Eq {
                        "IS NULL"
                    } else {
                        "IS NOT NULL"
                    };
                    format!("{lhs} {nullness}")
                } else {
                    // An enum column compares against a bare variant, which lowers to its
                    // wire string literal (not a column reference).
                    let rhs = self
                        .enum_variant_lit(model, path, value)
                        .unwrap_or_else(|| self.value(value, model));
                    match op {
                        // Collection ops don't fit plain infix: `in` needs a value list,
                        // `has` is JSON-array containment — MySQL's `value MEMBER OF(arr)`
                        // vs. Postgres's `arr @> value` (the JSONB containment operator).
                        Op::In => format!("{lhs} IN ({rhs})"),
                        Op::Has => match self.dialect {
                            Dialect::Postgres => format!("{lhs} @> {rhs}"),
                            _ => format!("{rhs} MEMBER OF({lhs})"),
                        },
                        _ => format!("{lhs} {} {rhs}", sql_op(*op)),
                    }
                };
                // A `?` optional param on the RHS present-guards this leaf so it drops when
                // the arg is absent — the piece that makes an or-composed optional filter work.
                self.guard_optional(value, pred)
            }
            // `path in (v, v, …)`: each element lowers like an equality RHS — an
            // enum variant to its wire value, a `$param` to its own placeholder
            // (bound positionally), a literal per-dialect.
            Predicate::InList { path, values } => {
                let (alias, col) = self.resolve(path, model);
                let lhs = self.qcol(&alias, &col);
                let mut items = Vec::with_capacity(values.len());
                for v in values {
                    let item = self
                        .enum_variant_lit(model, path, v)
                        .unwrap_or_else(|| self.value(v, model));
                    items.push(item);
                }
                format!("{lhs} IN ({})", items.join(", "))
            }
            // A bare atom is either a zero-arg named filter or a plain bool column.
            Predicate::Bare(path) => {
                if path.segments.len() == 1 {
                    if let Some(f) = self.filters.get(path.segments[0].node.as_str()).copied() {
                        return self.filter_call(f, &[], model);
                    }
                }
                let (alias, col) = self.resolve(path, model);
                format!(
                    "{} = {}",
                    self.qcol(&alias, &col),
                    self.dialect.bool_lit(true)
                )
            }
            // Inline the filter's body, substituting args for its params, resolved
            // against the call-site model (sema guarantees the body resolves).
            Predicate::FilterCall { name, args } => match self.filters.get(name.node.as_str()) {
                Some(f) => self.filter_call(f, args, model),
                None => format!("TRUE /* filter {} unresolved */", name.node),
            },
            Predicate::Raw(raw) => format!(
                "({})",
                render_raw(self.dialect, raw, &self.root_alias, &model.table)
            ),
        }
    }

    /// Inline a named filter: bind its params to the call arguments, substitute those
    /// bindings through its body, then lower the result against `model`. The filter
    /// carries no model of its own, so its column paths resolve at the call site.
    fn filter_call(&mut self, f: &'a NamedFilter, args: &[Value], model: &RModel) -> String {
        // Recursion guard: a self-referential filter is legal (sema terminates it);
        // stop re-expanding and leave a visible marker rather than looping.
        if self.filter_stack.contains(&f.name.node.as_str()) {
            return format!("TRUE /* filter {} recursion */", f.name.node);
        }
        // Arity is enforced by sema (E0115); guard defensively against a mismatch.
        if f.params.len() != args.len() {
            return format!("TRUE /* filter {} arity */", f.name.node);
        }
        let binds: HashMap<&str, &Value> = f
            .params
            .iter()
            .map(|p| p.name.node.as_str())
            .zip(args)
            .collect();
        let body = subst_pred(&f.pred, &binds);
        self.filter_stack.push(f.name.node.as_str());
        let sql = self.predicate(&body, model);
        self.filter_stack.pop();
        format!("({sql})")
    }
}
