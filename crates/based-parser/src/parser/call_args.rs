use super::*;

impl<'a> Parser<'a> {
    /// `( [value { , value }] )` — a filter call's argument list, from the opening paren.
    pub(super) fn call_args(&mut self) -> PResult<Vec<Value>> {
        self.expect(Tok::LParen, "`(`")?;
        let mut args = Vec::new();
        if !self.at(Tok::RParen) {
            loop {
                args.push(self.value()?);
                if !self.eat(Tok::Comma) {
                    break;
                }
            }
        }
        self.expect(Tok::RParen, "`)`")?;
        Ok(args)
    }
}
