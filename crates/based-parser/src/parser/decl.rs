use super::*;

impl<'a> Parser<'a> {
    pub(super) fn decl(&mut self) -> PResult<Decl> {
        if self.at(Tok::At) || self.at(Tok::UpperIdent) {
            return self.model().map(Decl::Model);
        }
        match self.ident_at(0) {
            Some("shape") => self.shape().map(Decl::Shape),
            Some("scope") => self.scope_decl().map(Decl::Scope),
            Some("enum") => self.enum_decl().map(Decl::Enum),
            Some("query") => self.query().map(Decl::Query),
            Some("mutation") => self.mutation().map(Decl::Mutation),
            Some("filter") => self.named_filter().map(Decl::Filter),
            _ => {
                self.err("expected a declaration (model, shape, query, mutation, or filter)");
                Err(())
            }
        }
    }
}
