//! Live smoke for the helpdesk service. Boots the axum app in-process against the
//! seeded Postgres at `DATABASE_URL`, then drives the whole surface over real HTTP:
//! auth + tenant isolation, the scoped lists, the queue, idempotent open, the close
//! guard (deny and allow), archive/restore, drafts (AND scope), the raw-SQL report,
//! the cross-tenant admin listing, and the NDJSON export (including a client that
//! disconnects mid-stream). Exits 0 only if every gate passes.
//!
//! `cargo run --bin smoke -- reset` instead drops and recreates the `public`
//! schema — the fresh-database step the CI target runs before `based migrate apply`
//! and the seed.

use std::future::IntoFuture;

use axum_helpdesk::{routes, App};
use futures_util::StreamExt;
use serde_json::{json, Value};

const ADA: &str = "tok-acme-ada"; // acme requester
const MARA: &str = "tok-acme-mara"; // acme agent
const NOAH: &str = "tok-acme-noah"; // acme agent
const GUS: &str = "tok-globex-gus"; // globex agent
const GRETA: &str = "tok-globex-greta"; // globex requester

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL (see .env)");
    if std::env::args().nth(1).as_deref() == Some("reset") {
        reset(&url).await;
        return;
    }

    // Boot the real service on an ephemeral loopback port; every probe below is a
    // genuine HTTP round-trip through the axum stack.
    let app = App::connect(&url).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind an ephemeral port");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(axum::serve(listener, routes::router(app)).into_future());
    let desk = Desk {
        http: reqwest::Client::new(),
        base: format!("http://{addr}"),
    };

    // ---- auth: 401 without a token, 401 on an unknown one ---------------------
    let (status, body) = desk.get(None, "/my/tickets").await;
    assert_eq!(status, 401, "{body}");
    assert_eq!(body["error"]["code"], "unauthorized");
    let (status, _) = desk.get(Some("tok-nope"), "/my/tickets").await;
    assert_eq!(status, 401);
    println!("ok - missing/unknown bearer token -> 401");

    // ---- requester portal: each tenant's requester sees only their own --------
    let (status, ada_tickets) = desk.get(Some(ADA), "/my/tickets").await;
    assert_eq!(status, 200, "{ada_tickets}");
    assert_eq!(ada_tickets.as_array().unwrap().len(), 3);
    assert!(ada_tickets
        .as_array()
        .unwrap()
        .iter()
        .all(|t| t["requester_name"] == "Ada"));
    let t1 = find(&ada_tickets, "subject", "Password reset loop");
    let t2 = find(&ada_tickets, "subject", "Invoice PDF won't download");

    let (status, greta_tickets) = desk.get(Some(GRETA), "/my/tickets").await;
    assert_eq!(status, 200, "{greta_tickets}");
    assert_eq!(greta_tickets.as_array().unwrap().len(), 1);
    let g1 = greta_tickets[0]["id"].as_str().unwrap().to_string();
    let ada_ids: Vec<&str> = ada_tickets
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap())
        .collect();
    assert!(!ada_ids.contains(&g1.as_str()));
    println!("ok - two tenants' requesters see disjoint tickets");

    // ---- role gate: a requester has no desk ------------------------------------
    let (status, body) = desk.get(Some(ADA), "/queue").await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(body["error"]["code"], "forbidden");
    println!("ok - requester hitting the desk -> 403 forbidden");

    // ---- scoped search: default status filter, wire enum values, isolation ----
    let (status, page) = desk.get(Some(MARA), "/tickets").await;
    assert_eq!(status, 200, "{page}");
    let rows = page["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 3, "acme's open tickets: {page}");
    // `order (priority desc, …)`: the urgent one leads.
    assert_eq!(rows[0]["subject"], "Data export is empty");
    assert_eq!(rows[0]["priority"], 4);
    let t4 = rows[0]["id"].as_str().unwrap().to_string();

    // The string enum's stored value is the wire value ("waiting_on_customer"),
    // even though the schema spells the variant `waiting`.
    let (status, page) = desk
        .get(Some(MARA), "/tickets?status=waiting_on_customer")
        .await;
    assert_eq!(status, 200, "{page}");
    assert_eq!(page["rows"].as_array().unwrap().len(), 1);
    assert_eq!(page["rows"][0]["id"].as_str().unwrap(), t2);

    let (status, page) = desk.get(Some(GUS), "/tickets").await;
    assert_eq!(status, 200, "{page}");
    assert_eq!(page["rows"].as_array().unwrap().len(), 1);
    assert_eq!(page["rows"][0]["id"].as_str().unwrap(), g1);

    let (status, body) = desk.get(Some(MARA), "/tickets?cursor=garbage").await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], "bad_cursor");
    println!("ok - scoped search: status filter, sort order, tenant isolation, bad cursor -> 400");

    // ---- the queue: priority >= high, assigned to the caller ------------------
    let (status, queue) = desk.get(Some(MARA), "/queue").await;
    assert_eq!(status, 200, "{queue}");
    assert_eq!(queue.as_array().unwrap().len(), 1, "{queue}");
    assert_eq!(queue[0]["id"].as_str().unwrap(), t1);
    println!("ok - queue: priority >= high and assignee = $ctx.user");

    // ---- ticket detail: nested shapes with ordered to-many arrays -------------
    let (status, detail) = desk.get(Some(MARA), &format!("/tickets/{t1}")).await;
    assert_eq!(status, 200, "{detail}");
    assert_eq!(detail["requester"]["name"], "Ada");
    assert_eq!(detail["assignee"]["name"], "Mara");
    assert_eq!(detail["duplicates"].as_array().unwrap().len(), 1);
    assert_eq!(detail["comments"].as_array().unwrap().len(), 3);
    let commented: Vec<&str> = detail["comments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["created_at"].as_str().unwrap())
        .collect();
    assert!(sorted(&commented), "comments oldest-first: {commented:?}");
    // The seed logged these out of chronological order on purpose.
    let logged: Vec<&str> = detail["time_entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["logged_at"].as_str().unwrap())
        .collect();
    assert_eq!(logged.len(), 3);
    assert!(sorted(&logged), "time entries by logged_at: {logged:?}");
    assert_eq!(detail["time_entries"][0]["note"], "first triage");
    println!("ok - detail: nested to-one shapes + to-many arrays in declared sort order");

    // ---- cross-tenant read: another tenant's ticket does not exist here -------
    let (status, body) = desk.get(Some(MARA), &format!("/tickets/{g1}")).await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(body["error"]["code"], "not_found");
    println!("ok - cross-tenant ticket read -> 404");

    // ---- cross-tenant write: an update that matches no row is a clean 404 -----
    let (status, body) = desk
        .post(
            Some(GUS),
            &format!("/tickets/{t1}/status"),
            json!({ "status": "waiting_on_customer" }),
            None,
        )
        .await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(body["error"]["code"], "not_found");
    println!("ok - cross-tenant status update -> 404, nothing written");

    // ---- idempotent open: one key, one row -------------------------------------
    let open = json!({ "subject": "Printer on fire", "body": "Actual flames.", "priority": 3 });
    let (status, first) = desk
        .post(Some(ADA), "/tickets", open.clone(), Some("smoke-open-1"))
        .await;
    assert_eq!(status, 201, "{first}");
    let (status, replay) = desk
        .post(Some(ADA), "/tickets", open.clone(), Some("smoke-open-1"))
        .await;
    assert_eq!(status, 201, "{replay}");
    assert_eq!(
        first["id"], replay["id"],
        "the retry replays, never re-writes"
    );
    let (_, after) = desk.get(Some(ADA), "/my/tickets").await;
    assert_eq!(after.as_array().unwrap().len(), 4, "exactly one new row");
    // The opening comment landed in the same transaction as the ticket.
    assert_eq!(first["comments"].as_array().unwrap().len(), 1);
    // The same key with a different request is a loud 422, never a replay.
    let (status, body) = desk
        .post(
            Some(ADA),
            "/tickets",
            json!({ "subject": "Different", "body": "request" }),
            Some("smoke-open-1"),
        )
        .await;
    assert_eq!(status, 422, "{body}");
    assert_eq!(body["error"]["code"], "idempotency_key_reuse");
    println!("ok - Idempotency-Key: replayed open, one row, reuse -> 422");

    // ---- the close guard: deny while unresolved, allow once resolved ----------
    let (status, body) = desk
        .post(Some(MARA), &format!("/tickets/{t1}/close"), json!({}), None)
        .await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(body["error"]["code"], "guard_denied");
    assert_eq!(
        body["error"]["message"],
        "only a resolved ticket can be closed"
    );
    // Another tenant's agent can't even see the ticket — same guard, same 403.
    let (status, body) = desk
        .post(Some(GUS), &format!("/tickets/{t1}/close"), json!({}), None)
        .await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(body["error"]["code"], "guard_denied");
    let (status, row) = desk
        .post(
            Some(MARA),
            &format!("/tickets/{t1}/status"),
            json!({ "status": "resolved" }),
            None,
        )
        .await;
    assert_eq!(status, 200, "{row}");
    let (status, row) = desk
        .post(Some(MARA), &format!("/tickets/{t1}/close"), json!({}), None)
        .await;
    assert_eq!(status, 200, "{row}");
    assert_eq!(row["status"], "closed");
    println!("ok - close guard: 403 guard_denied while unresolved, allowed once resolved");

    // ---- archive / restore (soft delete round trip) -----------------------------
    let (status, _) = desk.delete(Some(MARA), &format!("/tickets/{t2}")).await;
    assert_eq!(status, 200);
    let (_, mine) = desk.get(Some(ADA), "/my/tickets").await;
    assert_eq!(
        mine.as_array().unwrap().len(),
        3,
        "the archived one is gone"
    );
    let (status, _) = desk
        .post(
            Some(MARA),
            &format!("/tickets/{t2}/restore"),
            json!({}),
            None,
        )
        .await;
    assert_eq!(status, 200);
    let (_, mine) = desk.get(Some(ADA), "/my/tickets").await;
    assert_eq!(mine.as_array().unwrap().len(), 4, "and back");
    println!("ok - archive tombstones, restore lifts it");

    // ---- comments + billable time land and come back in order ------------------
    let (status, comment) = desk
        .post(
            Some(ADA),
            &format!("/tickets/{t2}/comments"),
            json!({ "body": "Any update?" }),
            None,
        )
        .await;
    assert_eq!(status, 200, "{comment}");
    assert_eq!(comment["author"]["name"], "Ada");
    let (status, entry) = desk
        .post(
            Some(MARA),
            &format!("/tickets/{t2}/time"),
            json!({
                "hours": 0.25,
                "amount": "23.75",
                "note": "triage",
                "logged_at": "2026-07-10 09:00:00+00"
            }),
            None,
        )
        .await;
    assert_eq!(status, 200, "{entry}");
    assert_eq!(entry["amount"], "23.75", "decimal rides as an exact string");
    println!("ok - comment + billable time (float hours, exact decimal amount)");

    // ---- purge (the `-> ok` hard delete): ack on success, 404 once gone --------
    let cid = comment["id"].as_str().unwrap().to_string();
    // A requester has no purge desk.
    let (status, _) = desk
        .delete(Some(ADA), &format!("/admin/comments/{cid}"))
        .await;
    assert_eq!(status, 403);
    // Another tenant's agent gets the same 404 an absent row would — and deletes
    // nothing (no existence leak across the scope boundary).
    let (status, body) = desk
        .delete(Some(GUS), &format!("/admin/comments/{cid}"))
        .await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(body["error"]["code"], "not_found");
    // The tenant's own agent purges it: a bare 200 acknowledgement, no body.
    let (status, body) = desk
        .delete(Some(MARA), &format!("/admin/comments/{cid}"))
        .await;
    assert_eq!(status, 200, "{body}");
    // Purging the same id again matches nothing: the engine's own 404.
    let (status, body) = desk
        .delete(Some(MARA), &format!("/admin/comments/{cid}"))
        .await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(body["error"]["code"], "not_found");
    println!("ok - purge comment: hard delete acks with 200, cross-tenant/re-purge -> 404");

    // ---- drafts: the AND scope (org AND author) ---------------------------------
    let (status, drafts) = desk.get(Some(MARA), &format!("/tickets/{t1}/drafts")).await;
    assert_eq!(status, 200, "{drafts}");
    assert_eq!(drafts.as_array().unwrap().len(), 1, "Mara's seeded draft");
    let (_, drafts) = desk.get(Some(NOAH), &format!("/tickets/{t1}/drafts")).await;
    assert_eq!(
        drafts.as_array().unwrap().len(),
        0,
        "same org, not the author"
    );
    let (status, _) = desk
        .post(
            Some(NOAH),
            &format!("/tickets/{t1}/drafts"),
            json!({ "body": "Check the mail previewer theory." }),
            None,
        )
        .await;
    assert_eq!(status, 200);
    let (_, drafts) = desk.get(Some(NOAH), &format!("/tickets/{t1}/drafts")).await;
    assert_eq!(drafts.as_array().unwrap().len(), 1, "Noah now sees his own");
    let (_, drafts) = desk.get(Some(MARA), &format!("/tickets/{t1}/drafts")).await;
    assert_eq!(
        drafts.as_array().unwrap().len(),
        1,
        "Mara still sees only hers"
    );
    println!("ok - drafts: AND scope isolates by org and author");

    // ---- tag containment + per-param bindings -----------------------------------
    let (status, tagged) = desk.get(Some(MARA), "/tags/vip/tickets").await;
    assert_eq!(status, 200, "{tagged}");
    assert_eq!(tagged.as_array().unwrap().len(), 1);
    assert_eq!(tagged[0]["id"].as_str().unwrap(), t4);

    let (_, t4_detail) = desk.get(Some(MARA), &format!("/tickets/{t4}")).await;
    let noah_id = t4_detail["assignee"]["id"].as_str().unwrap().to_string();
    let (status, noahs) = desk
        .get(Some(MARA), &format!("/agents/{noah_id}/tickets"))
        .await;
    assert_eq!(status, 200, "{noahs}");
    assert_eq!(noahs.as_array().unwrap().len(), 1);
    assert_eq!(noahs[0]["id"].as_str().unwrap(), t4);
    println!("ok - tags has + per-param bindings (agent -> assignee, since > created_at)");

    // ---- workload report: raw-SQL rollups over the engine-owned row set --------
    let (status, report) = desk.get(Some(MARA), "/reports/workload").await;
    assert_eq!(status, 200, "{report}");
    let agents = report.as_array().unwrap();
    assert_eq!(agents.len(), 2, "acme's two agents, sorted by name");
    assert_eq!(agents[0]["name"], "Mara");
    assert_eq!(agents[0]["rate"], "95.00");
    let noah = &agents[1];
    assert_eq!(noah["name"], "Noah");
    assert_eq!(noah["open_tickets"], 1, "t4 is open and Noah's: {noah}");
    assert_eq!(noah["hours_logged"].as_f64(), Some(1.0));
    println!("ok - workload report: raw-SQL aggregate leaves");

    // ---- admin listing: unscoped, offset-paged ----------------------------------
    let (status, all) = desk.get(Some(MARA), "/admin/tickets").await;
    assert_eq!(status, 200, "{all}");
    let subjects: Vec<&str> = all["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["subject"].as_str().unwrap())
        .collect();
    assert!(
        subjects.contains(&"Webhook retries flooding us"),
        "{subjects:?}"
    );
    assert!(subjects.contains(&"Printer on fire"), "{subjects:?}");
    let total = subjects.len();
    assert_eq!(
        all["total"].as_i64(),
        Some(total as i64),
        "`with count` serves the total: {all}"
    );
    let (_, offset) = desk.get(Some(MARA), "/admin/tickets?offset=2").await;
    assert_eq!(offset["rows"].as_array().unwrap().len(), total - 2);
    assert_ne!(offset["rows"][0]["id"], all["rows"][0]["id"]);
    assert_eq!(
        offset["total"].as_i64(),
        Some(total as i64),
        "the total counts the whole live set, not the window: {offset}"
    );
    println!("ok - admin listing: cross-tenant (unscoped) + offset pagination + total");

    // ---- the NDJSON export ------------------------------------------------------
    let resp = desk
        .http
        .get(format!("{}/export/tickets.ndjson", desk.base))
        .bearer_auth(MARA)
        .send()
        .await
        .expect("export request");
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()["content-type"],
        "application/x-ndjson",
        "the streaming content type"
    );
    let text = resp.text().await.expect("export body");
    let lines: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).expect("one JSON envelope per line"))
        .collect();
    let (terminal, rows) = lines.split_last().expect("a non-empty body");
    assert!(rows.iter().all(|l| l.get("row").is_some()));
    assert_eq!(
        terminal["done"]["rows"].as_u64(),
        Some(rows.len() as u64),
        "the terminal line carries the row-count checksum"
    );
    assert_eq!(rows.len(), total, "the export walks every live ticket");
    let orgs: Vec<&str> = rows
        .iter()
        .map(|l| l["row"]["org"].as_str().unwrap())
        .collect();
    assert!(orgs.contains(&"acme") && orgs.contains(&"globex"));
    assert!(rows[0]["row"]["age_days"].is_number(), "the raw-SQL leaf");
    println!("ok - NDJSON export: rows + terminal done checksum, cross-tenant, raw age_days");

    // ---- a client that walks away mid-export: cancel, not a leak ---------------
    let resp = desk
        .http
        .get(format!("{}/export/tickets.ndjson", desk.base))
        .bearer_auth(MARA)
        .send()
        .await
        .expect("second export request");
    let mut body = resp.bytes_stream();
    let _first = body.next().await;
    drop(body); // hang up mid-stream — the server must cancel the pass
    let (status, _) = desk.get(Some(MARA), "/queue").await;
    assert_eq!(status, 200, "the pool is healthy after a dropped stream");
    println!("ok - dropped export stream: connection recovered, next request green");

    println!("\nsmoke: every gate green");
}

// ---- helpers -----------------------------------------------------------------

struct Desk {
    http: reqwest::Client,
    base: String,
}

impl Desk {
    async fn get(&self, token: Option<&str>, path: &str) -> (u16, Value) {
        let mut req = self.http.get(format!("{}{path}", self.base));
        if let Some(token) = token {
            req = req.bearer_auth(token);
        }
        Self::finish(req).await
    }

    async fn post(
        &self,
        token: Option<&str>,
        path: &str,
        body: Value,
        idempotency_key: Option<&str>,
    ) -> (u16, Value) {
        let mut req = self.http.post(format!("{}{path}", self.base)).json(&body);
        if let Some(token) = token {
            req = req.bearer_auth(token);
        }
        if let Some(key) = idempotency_key {
            req = req.header("Idempotency-Key", key);
        }
        Self::finish(req).await
    }

    async fn delete(&self, token: Option<&str>, path: &str) -> (u16, Value) {
        let mut req = self.http.delete(format!("{}{path}", self.base));
        if let Some(token) = token {
            req = req.bearer_auth(token);
        }
        Self::finish(req).await
    }

    async fn finish(req: reqwest::RequestBuilder) -> (u16, Value) {
        let resp = req.send().await.expect("http round-trip");
        let status = resp.status().as_u16();
        let body = resp.json().await.unwrap_or(Value::Null);
        (status, body)
    }
}

/// The id of the row whose `field` equals `value` — panics loudly when absent.
fn find(rows: &Value, field: &str, value: &str) -> String {
    rows.as_array()
        .unwrap()
        .iter()
        .find(|r| r[field] == value)
        .unwrap_or_else(|| panic!("no row with {field} = {value:?} in {rows}"))["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn sorted(values: &[&str]) -> bool {
    values.windows(2).all(|w| w[0] <= w[1])
}

/// Drop and recreate the `public` schema — a fresh database for `based migrate
/// apply` + the seed, so the smoke is re-runnable on a shared throwaway server.
async fn reset(url: &str) {
    use sqlx::Connection;
    let mut conn = sqlx::postgres::PgConnection::connect(url)
        .await
        .unwrap_or_else(|e| panic!("connect to Postgres at {url}: {e}"));
    sqlx::raw_sql("drop schema public cascade; create schema public;")
        .execute(&mut conn)
        .await
        .expect("reset schema");
    println!("smoke: database reset (schema public recreated)");
}
