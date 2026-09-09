use super::*;

impl<'a> Parser<'a> {
    pub(super) fn sort_term(&mut self) -> PResult<SortTerm> {
        let path = self.path()?;
        let dir = if self.eat_kw("desc") {
            SortDir::Desc
        } else {
            self.eat_kw("asc");
            SortDir::Asc
        };
        let nulls = if self.eat_kw("nulls") {
            if self.eat_kw("first") {
                Some(NullsPlacement::First)
            } else {
                self.eat_kw("last");
                Some(NullsPlacement::Last)
            }
        } else {
            None
        };
        Ok(SortTerm { path, dir, nulls })
    }
}
