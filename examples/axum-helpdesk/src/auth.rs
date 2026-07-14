//! Bearer-token auth. A request's `Authorization: Bearer <token>` is traded for a
//! typed session **through the client itself** (`session_by_token`, the one
//! `unscoped` auth query in the schema) — so everything handlers later pass as
//! `$ctx` is derived server-side, never read from a request body.

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::client::{Role, SessionByTokenInput, SessionCtx};
use crate::routes::ApiError;
use crate::App;

/// Resolve the bearer token into the caller's [`SessionCtx`] and ride it on the
/// request as an extension; `401` when the token is missing or unknown.
pub async fn require_session(State(app): State<App>, mut req: Request, next: Next) -> Response {
    let token = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let Some(token) = token else {
        return ApiError::unauthorized("missing bearer token").into_response();
    };
    let session = app
        .api()
        .session_by_token(
            SessionByTokenInput {
                token: token.to_string(),
            },
            (),
        )
        .await;
    match session {
        Ok(Some(session)) => {
            req.extensions_mut().insert(session);
            next.run(req).await
        }
        Ok(None) => ApiError::unauthorized("unknown bearer token").into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

/// Desk and ops routes are staff-only; requesters keep to the portal.
pub fn require_agent(session: &SessionCtx) -> Result<(), ApiError> {
    match session.role {
        Role::Agent | Role::Admin => Ok(()),
        Role::Requester => Err(ApiError::forbidden("agents only")),
    }
}
