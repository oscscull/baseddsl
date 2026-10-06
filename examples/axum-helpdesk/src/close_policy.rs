//! Preflight close policy; the schema write enforces the state transition.

use crate::client;
use based_runtime::{GuardRequest, GuardVerdict};

/// A preflight check: deny a ticket that is not currently visible and resolved.
/// The schema's conditional UPDATE enforces resolved-to-closed atomically; this
/// read runs separately from the write, including with an adopted transaction.
/// The decision is app code — but the *read*
/// it decides on goes back through the schema's own `ticket` query over `req.engine()`,
/// so the workspace scope and the soft-delete filter are the ones the schema declares.
/// A check that cannot decide denies — fail closed.
pub async fn caller_can_close(req: GuardRequest) -> GuardVerdict {
    let (Ok(input), Ok(ctx)) = (
        serde_json::from_value::<client::TicketInput>(req.args.clone()),
        serde_json::from_value::<client::TicketCtx>(req.ctx.clone()),
    ) else {
        return GuardVerdict::deny("close requires a ticket id and a workspace");
    };
    match client::embedded(req.engine()).ticket(input, ctx).await {
        Ok(Some(t)) if t.status == client::Status::Resolved => GuardVerdict::Allow,
        Ok(Some(_)) => GuardVerdict::deny("only a resolved ticket can be closed"),
        Ok(None) => GuardVerdict::deny("no such ticket in this workspace"),
        Err(_) => GuardVerdict::deny("could not verify the ticket"),
    }
}
