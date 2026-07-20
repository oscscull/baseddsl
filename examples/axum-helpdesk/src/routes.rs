//! The HTTP surface: every handler is a thin translation from an HTTP request to
//! one typed client call — path/query/body in, the session's `$ctx` alongside,
//! JSON (or NDJSON, for the export) back out.

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::json;

use crate::auth::{self, require_agent};
use crate::client::{self, entity, Cursor, Id, SessionCtx, Status};
use crate::App;

pub fn router(app: App) -> Router {
    Router::new()
        // requester portal
        .route("/tickets", post(open_ticket).get(search_tickets))
        .route("/my/tickets", get(my_tickets))
        .route("/tickets/{id}/comments", post(add_comment))
        // agent desk
        .route("/tickets/{id}", get(ticket).delete(archive_ticket))
        .route("/queue", get(queue))
        .route("/agents/{id}/tickets", get(tickets_for))
        .route("/tags/{tag}/tickets", get(tagged_tickets))
        .route("/tickets/{id}/assign", post(assign_ticket))
        .route("/tickets/{id}/status", post(set_status))
        .route("/tickets/{id}/tags", post(tag_ticket))
        .route("/tickets/{id}/duplicate", post(mark_duplicate))
        .route("/tickets/{id}/close", post(close_ticket))
        .route("/tickets/{id}/restore", post(restore_ticket))
        .route("/tickets/{id}/time", post(log_time))
        .route("/tickets/{id}/drafts", get(my_drafts).post(save_draft))
        // ops / finance
        .route("/export/tickets.ndjson", get(export_tickets))
        .route("/reports/workload", get(workload_report))
        .route("/admin/tickets", get(admin_tickets))
        .route("/admin/comments/{id}", delete(purge_comment))
        .layer(middleware::from_fn_with_state(
            app.clone(),
            auth::require_session,
        ))
        .with_state(app)
}

// ---- requester portal -------------------------------------------------------

async fn open_ticket(
    State(app): State<App>,
    Extension(s): Extension<SessionCtx>,
    headers: HeaderMap,
    Json(input): Json<client::OpenTicketInput>,
) -> Result<(StatusCode, Json<client::TicketDetail>), ApiError> {
    let ctx = client::OpenTicketCtx {
        org: s.org,
        user: s.user,
    };
    let api = app.api();
    // The standard retry contract: a caller supplying `Idempotency-Key` gets the
    // keyed twin — a retried POST replays the first response, one row ever.
    let ticket = match headers.get("idempotency-key").and_then(|v| v.to_str().ok()) {
        Some(key) => api.open_ticket_with_key(input, ctx, key).await?,
        None => api.open_ticket(input, ctx).await?,
    };
    Ok((StatusCode::CREATED, Json(ticket)))
}

async fn my_tickets(
    State(app): State<App>,
    Extension(s): Extension<SessionCtx>,
) -> Result<Json<Vec<client::TicketRow>>, ApiError> {
    let rows = app
        .api()
        .my_tickets(
            client::MyTicketsInput,
            client::MyTicketsCtx { user: s.user },
        )
        .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
struct CommentBody {
    body: String,
}

async fn add_comment(
    State(app): State<App>,
    Extension(s): Extension<SessionCtx>,
    Path(id): Path<Id<entity::Ticket>>,
    Json(b): Json<CommentBody>,
) -> Result<Json<client::CommentRow>, ApiError> {
    let row = app
        .api()
        .add_comment(
            client::AddCommentInput {
                ticket: id,
                body: b.body,
            },
            client::AddCommentCtx {
                org: s.org,
                user: s.user,
            },
        )
        .await?;
    Ok(Json(row))
}

// ---- agent desk --------------------------------------------------------------

#[derive(Deserialize)]
struct SearchParams {
    #[serde(default)]
    q: String,
    status: Option<Status>,
    cursor: Option<Cursor>,
}

async fn search_tickets(
    State(app): State<App>,
    Extension(s): Extension<SessionCtx>,
    Query(p): Query<SearchParams>,
) -> Result<Json<client::Page<client::TicketRow>>, ApiError> {
    require_agent(&s)?;
    let page = app
        .api()
        .search_tickets(
            client::SearchTicketsInput {
                // `~` is SQL LIKE; the route's `q` is a substring, so wrap it.
                q: format!("%{}%", p.q),
                status: p.status,
                cursor: p.cursor,
            },
            client::SearchTicketsCtx { org: s.org },
        )
        .await?;
    Ok(Json(page))
}

async fn ticket(
    State(app): State<App>,
    Extension(s): Extension<SessionCtx>,
    Path(id): Path<Id<entity::Ticket>>,
) -> Result<Json<client::TicketDetail>, ApiError> {
    require_agent(&s)?;
    let detail = app
        .api()
        .ticket(client::TicketInput { id }, client::TicketCtx { org: s.org })
        .await?;
    detail
        .map(Json)
        .ok_or_else(|| ApiError::not_found("ticket"))
}

async fn queue(
    State(app): State<App>,
    Extension(s): Extension<SessionCtx>,
) -> Result<Json<Vec<client::TicketRow>>, ApiError> {
    require_agent(&s)?;
    let rows = app
        .api()
        .queue(
            client::QueueInput,
            client::QueueCtx {
                org: s.org,
                user: s.user,
            },
        )
        .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
struct SinceParam {
    since: Option<String>,
}

/// The epoch, in the timestamp spelling the engine accepts — "no lower bound".
const BEGINNING: &str = "1970-01-01 00:00:00+00";

async fn tickets_for(
    State(app): State<App>,
    Extension(s): Extension<SessionCtx>,
    Path(id): Path<Id<entity::User>>,
    Query(p): Query<SinceParam>,
) -> Result<Json<Vec<client::TicketRow>>, ApiError> {
    require_agent(&s)?;
    let rows = app
        .api()
        .tickets_for(
            client::TicketsForInput {
                agent: id,
                since: p.since.unwrap_or_else(|| BEGINNING.into()),
            },
            client::TicketsForCtx { org: s.org },
        )
        .await?;
    Ok(Json(rows))
}

async fn tagged_tickets(
    State(app): State<App>,
    Extension(s): Extension<SessionCtx>,
    Path(tag): Path<String>,
) -> Result<Json<Vec<client::TicketRow>>, ApiError> {
    require_agent(&s)?;
    let rows = app
        .api()
        .tagged_tickets(
            client::TaggedTicketsInput { tag: json!(tag) },
            client::TaggedTicketsCtx { org: s.org },
        )
        .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
struct AssignBody {
    agent: Id<entity::User>,
}

async fn assign_ticket(
    State(app): State<App>,
    Extension(s): Extension<SessionCtx>,
    Path(id): Path<Id<entity::Ticket>>,
    Json(b): Json<AssignBody>,
) -> Result<Json<client::TicketRow>, ApiError> {
    require_agent(&s)?;
    let row = app
        .api()
        .assign_ticket(
            client::AssignTicketInput { id, agent: b.agent },
            client::AssignTicketCtx { org: s.org },
        )
        .await?;
    Ok(Json(row))
}

#[derive(Deserialize)]
struct StatusBody {
    status: Status,
}

async fn set_status(
    State(app): State<App>,
    Extension(s): Extension<SessionCtx>,
    Path(id): Path<Id<entity::Ticket>>,
    Json(b): Json<StatusBody>,
) -> Result<Json<client::TicketRow>, ApiError> {
    require_agent(&s)?;
    let row = app
        .api()
        .set_status(
            client::SetStatusInput {
                id,
                status: b.status,
            },
            client::SetStatusCtx { org: s.org },
        )
        .await?;
    Ok(Json(row))
}

#[derive(Deserialize)]
struct TagsBody {
    tags: serde_json::Value,
}

async fn tag_ticket(
    State(app): State<App>,
    Extension(s): Extension<SessionCtx>,
    Path(id): Path<Id<entity::Ticket>>,
    Json(b): Json<TagsBody>,
) -> Result<Json<client::TicketRow>, ApiError> {
    require_agent(&s)?;
    let row = app
        .api()
        .tag_ticket(
            client::TagTicketInput { id, tags: b.tags },
            client::TagTicketCtx { org: s.org },
        )
        .await?;
    Ok(Json(row))
}

#[derive(Deserialize)]
struct DuplicateBody {
    of: Id<entity::Ticket>,
}

async fn mark_duplicate(
    State(app): State<App>,
    Extension(s): Extension<SessionCtx>,
    Path(id): Path<Id<entity::Ticket>>,
    Json(b): Json<DuplicateBody>,
) -> Result<Json<client::TicketRow>, ApiError> {
    require_agent(&s)?;
    let row = app
        .api()
        .mark_duplicate(
            client::MarkDuplicateInput { id, of: b.of },
            client::MarkDuplicateCtx { org: s.org },
        )
        .await?;
    Ok(Json(row))
}

/// The guarded close: the engine runs `caller_can_close` (src/app.rs) before the
/// write; a denial arrives as the engine's own `403 guard_denied` and passes
/// through [`ApiError`] untouched.
async fn close_ticket(
    State(app): State<App>,
    Extension(s): Extension<SessionCtx>,
    Path(id): Path<Id<entity::Ticket>>,
) -> Result<Json<client::TicketRow>, ApiError> {
    require_agent(&s)?;
    let row = app
        .api()
        .close_ticket(
            client::CloseTicketInput { id },
            client::CloseTicketCtx { org: s.org },
        )
        .await?;
    Ok(Json(row))
}

async fn archive_ticket(
    State(app): State<App>,
    Extension(s): Extension<SessionCtx>,
    Path(id): Path<Id<entity::Ticket>>,
) -> Result<Json<client::TicketRow>, ApiError> {
    require_agent(&s)?;
    let row = app
        .api()
        .archive_ticket(
            client::ArchiveTicketInput { id },
            client::ArchiveTicketCtx { org: s.org },
        )
        .await?;
    Ok(Json(row))
}

async fn restore_ticket(
    State(app): State<App>,
    Extension(s): Extension<SessionCtx>,
    Path(id): Path<Id<entity::Ticket>>,
) -> Result<Json<client::TicketRow>, ApiError> {
    require_agent(&s)?;
    let row = app
        .api()
        .restore_ticket(
            client::RestoreTicketInput { id },
            client::RestoreTicketCtx { org: s.org },
        )
        .await?;
    Ok(Json(row))
}

#[derive(Deserialize)]
struct TimeBody {
    hours: f64,
    amount: rust_decimal::Decimal,
    note: String,
    logged_at: String,
}

async fn log_time(
    State(app): State<App>,
    Extension(s): Extension<SessionCtx>,
    Path(id): Path<Id<entity::Ticket>>,
    Json(b): Json<TimeBody>,
) -> Result<Json<client::TimeEntryRow>, ApiError> {
    require_agent(&s)?;
    let row = app
        .api()
        .log_time(
            client::LogTimeInput {
                ticket: id,
                hours: b.hours,
                amount: b.amount,
                note: b.note,
                logged_at: b.logged_at,
            },
            client::LogTimeCtx { user: s.user },
        )
        .await?;
    Ok(Json(row))
}

async fn my_drafts(
    State(app): State<App>,
    Extension(s): Extension<SessionCtx>,
    Path(id): Path<Id<entity::Ticket>>,
) -> Result<Json<Vec<client::DraftRow>>, ApiError> {
    require_agent(&s)?;
    let rows = app
        .api()
        .my_drafts(
            client::MyDraftsInput { ticket: id },
            client::MyDraftsCtx {
                org: s.org,
                user: s.user,
            },
        )
        .await?;
    Ok(Json(rows))
}

async fn save_draft(
    State(app): State<App>,
    Extension(s): Extension<SessionCtx>,
    Path(id): Path<Id<entity::Ticket>>,
    Json(b): Json<CommentBody>,
) -> Result<Json<client::DraftRow>, ApiError> {
    require_agent(&s)?;
    let row = app
        .api()
        .save_draft(
            client::SaveDraftInput {
                ticket: id,
                body: b.body,
            },
            client::SaveDraftCtx {
                org: s.org,
                user: s.user,
            },
        )
        .await?;
    Ok(Json(row))
}

// ---- ops / finance -----------------------------------------------------------

async fn export_tickets(
    State(app): State<App>,
    Extension(s): Extension<SessionCtx>,
    Query(p): Query<SinceParam>,
) -> Result<Response, ApiError> {
    require_agent(&s)?;
    let rows = app
        .api()
        .export_tickets(
            client::ExportTicketsInput {
                since: p.since.unwrap_or_else(|| BEGINNING.into()),
            },
            (),
        )
        .await?;
    Ok(ndjson(rows))
}

async fn workload_report(
    State(app): State<App>,
    Extension(s): Extension<SessionCtx>,
) -> Result<Json<Vec<client::AgentWorkload>>, ApiError> {
    require_agent(&s)?;
    let rows = app
        .api()
        .workload_report(client::WorkloadReportInput { org: s.org }, ())
        .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
struct OffsetParam {
    offset: Option<i64>,
}

async fn admin_tickets(
    State(app): State<App>,
    Extension(s): Extension<SessionCtx>,
    Query(p): Query<OffsetParam>,
) -> Result<Json<client::Page<client::TicketRow>>, ApiError> {
    require_agent(&s)?;
    let page = app
        .api()
        .admin_tickets(client::AdminTicketsInput { offset: p.offset }, ())
        .await?;
    Ok(Json(page))
}

/// Legal/PII removal: the `-> ok` hard delete. Success has no body to return —
/// a `200` acknowledgement; a missing (or cross-tenant) comment is the engine's
/// own `404 not_found`, passed through untouched.
async fn purge_comment(
    State(app): State<App>,
    Extension(s): Extension<SessionCtx>,
    Path(id): Path<Id<entity::Comment>>,
) -> Result<StatusCode, ApiError> {
    require_agent(&s)?;
    app.api()
        .purge_comment(
            client::PurgeCommentInput { id },
            client::PurgeCommentCtx { org: s.org },
        )
        .await?;
    Ok(StatusCode::OK)
}

// ---- streaming ---------------------------------------------------------------

/// Re-serve a typed row stream as NDJSON with the engine's wire framing: one
/// `{"row":…}` line per row, then exactly one terminal line — `{"done":{"rows":N}}`
/// on success, `{"error":{code,message}}` if the pass fails mid-stream (the `200`
/// is spent by then; the terminal line is the in-band verdict). A client that
/// disconnects drops the stream, which cancels the database pass.
fn ndjson<O: serde::Serialize + Send + 'static>(mut rows: client::RowStream<O>) -> Response {
    let lines = async_stream::stream! {
        let mut count: u64 = 0;
        while let Some(item) = rows.next().await {
            match item {
                Ok(row) => {
                    count += 1;
                    yield Ok::<_, std::convert::Infallible>(line(json!({ "row": row })));
                }
                Err(e) => {
                    yield Ok(line(
                        json!({ "error": { "code": e.code(), "message": e.message() } }),
                    ));
                    return;
                }
            }
        }
        yield Ok(line(json!({ "done": { "rows": count } })));
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-ndjson")
        .body(Body::from_stream(lines))
        .expect("static response parts are valid")
}

/// One NDJSON line: the envelope object, compact-serialized, newline-terminated.
fn line(envelope: serde_json::Value) -> Vec<u8> {
    let mut s = envelope.to_string();
    s.push('\n');
    s.into_bytes()
}

// ---- errors ------------------------------------------------------------------

/// One error shape for the whole surface: the engine's wire envelope
/// (`{ "error": { "code", "message" } }`) re-served with its real status, so an
/// HTTP caller reads the same failure an embedded caller would.
pub struct ApiError {
    status: StatusCode,
    code: String,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, code: &str, message: impl Into<String>) -> ApiError {
        ApiError {
            status,
            code: code.to_string(),
            message: message.into(),
        }
    }

    pub fn unauthorized(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::UNAUTHORIZED, "unauthorized", message)
    }

    pub fn forbidden(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::FORBIDDEN, "forbidden", message)
    }

    pub fn not_found(what: &str) -> ApiError {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("no such {what}"),
        )
    }
}

impl From<client::ClientError> for ApiError {
    fn from(e: client::ClientError) -> ApiError {
        match e.status() {
            // The engine's own verdict — guard 403, keyed 409/422, bad args 400,
            // database 503 — passes through with its stable code, untouched.
            Some(status) => ApiError::new(
                StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                e.code(),
                e.message(),
            ),
            // A transport/decode failure: the in-process call itself broke.
            None => ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.code(), e.message()),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = json!({ "error": { "code": self.code, "message": self.message } });
        (self.status, Json(body)).into_response()
    }
}
