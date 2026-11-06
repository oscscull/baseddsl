use super::*;

/// Validate a date-bucket call against its model: exactly one argument, naming a
/// `timestamp` or `date` column.
pub(crate) fn check_bucket_call(call: &BucketCall, mi: usize, cx: &Cx, sink: &mut Sink) {
    let name = call.bucket.name();
    let Some(arg) = call.arg() else {
        sink.error_note(
            code::DATE_BUCKET,
            call.span,
            format!(
                "`{name}` takes one column argument, got {}",
                call.args.len()
            ),
            format!("e.g. `{name}(created_at)`"),
        );
        return;
    };
    let Some(term) = resolve::resolve_path(arg, mi, cx, sink) else {
        return;
    };
    if resolve::reject_opaque(&term, arg, "bucket", sink) {
        return;
    }
    if !matches!(
        term,
        resolve::Terminal::Scalar(Primitive::Timestamp | Primitive::Date)
    ) {
        let span = arg.segments.last().map_or(call.span, |s| s.span);
        sink.error(
            code::DATE_BUCKET,
            span,
            format!(
                "`{name}` needs a `timestamp` or `date` column, not {}",
                resolve::terminal_name(&term)
            ),
        );
    }
}
