use super::*;

pub(crate) fn path_segments(path: &Path) -> Vec<&str> {
    path.segments.iter().map(|s| s.node.as_str()).collect()
}

pub(crate) fn find_filter<'a>(decls: &'a [Decl], name: &str) -> Option<&'a NamedFilter> {
    decls.iter().find_map(|d| match d {
        Decl::Filter(f) if f.name.node == name => Some(f),
        _ => None,
    })
}

/// Find a query decl by name.
pub(crate) fn find_query<'a>(decls: &'a [Decl], name: &str) -> Option<&'a Query> {
    decls.iter().find_map(|d| match d {
        Decl::Query(q) if q.name.node == name => Some(q),
        _ => None,
    })
}

/// Find a mutation decl by name.
pub(crate) fn find_mutation<'a>(decls: &'a [Decl], name: &str) -> Option<&'a Mutation> {
    decls.iter().find_map(|d| match d {
        Decl::Mutation(m) if m.name.node == name => Some(m),
        _ => None,
    })
}
