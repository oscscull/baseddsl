//! Inline a named filter: substitute its call arguments through its body, then lower it
//! against the call-site model.

use super::super::*;

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

impl<'a> Select<'a> {
    /// Inline a named filter: bind its params to the call arguments, substitute those
    /// bindings through its body, then lower the result against `model`. The filter
    /// carries no model of its own, so its column paths resolve at the call site.
    pub(super) fn filter_call(&mut self, f: &'a NamedFilter, args: &[Value], model: &RModel) -> String {
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
