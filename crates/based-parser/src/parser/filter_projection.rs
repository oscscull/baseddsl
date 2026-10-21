use super::*;

impl<'a> Parser<'a> {
    /// `name(args)` in a shape value — a named filter projected as a bool field.
    pub(super) fn filter_projection(&mut self) -> PResult<FilterProjection> {
        let name = self.lower_ident("filter name")?;
        let start = name.span.start;
        let args = self.call_args()?;
        Ok(FilterProjection {
            name,
            args,
            span: Span {
                file: self.file,
                start,
                end: self.prev_end(),
            },
        })
    }
}
