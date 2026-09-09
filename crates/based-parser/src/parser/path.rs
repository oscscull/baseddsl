use super::*;

impl<'a> Parser<'a> {
    pub(super) fn path(&mut self) -> PResult<Path> {
        let first = self.lower_ident("a path")?;
        Ok(self.path_from(first))
    }

    pub(super) fn path_from(&mut self, first: Ident) -> Path {
        let mut segments = vec![first];
        while self.eat(Tok::Dot) {
            match self.lower_ident("path segment") {
                Ok(seg) => segments.push(seg),
                Err(()) => break,
            }
        }
        Path { segments }
    }
}
