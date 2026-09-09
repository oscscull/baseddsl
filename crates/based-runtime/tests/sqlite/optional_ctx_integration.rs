//! End-to-end proof of an optional context read `$ctx.<field>?` (auth.md Handle 1) against a
//! **real** SQLite engine. The regression this guards: an absent field must lower to null-safe
//! equality (`col IS NULL`), *not* widen its leaf to TRUE — a widening leaf under an `or` would
//! collapse the group to TRUE and leak every private row the co-guard was gating.
//!
//! `Deck.user` is a **nullable** owner, so the run exercises both halves of the semantic:
//!   - an anonymous caller never sees another user's *private* deck (the leak that was fixed);
//!   - an anonymous caller *does* see an *unowned* deck (`user IS NULL` matches — "owned by no
//!     one, and you are no one"), the deliberate consequence of "unset context = unset column".

#![cfg(feature = "sqlite")]

use serde_json::json;

use based_ast::FileId;
use based_codegen::{sql, Dialect};
use based_parser::parse_file;
use based_runtime::idempotency::NoStore;
use based_runtime::{dispatch, Compiled, Guards, SeqIdGen, SqliteBackend};
use based_sema::check;

const SCHEMA: &str = r#"
@sort(id asc)
User { id: Id, name: text }
Deck { id: Id, user: User?, visibility: text, title: text }

shape DeckCard from Deck { title }

query deck_feed() -> DeckCard[] {
  list Deck where (user = $ctx.user? or visibility != "private");
}
"#;

// Two owners (u1, u2), plus two unowned decks; a mix of private/public.
const SEED: &str = r#"
INSERT INTO `user` (`id`, `name`) VALUES ('u1', 'Mac');
INSERT INTO `user` (`id`, `name`) VALUES ('u2', 'Rae');
INSERT INTO `deck` (`id`, `user_id`, `visibility`, `title`) VALUES ('d1', 'u1', 'private', 'MacPriv');
INSERT INTO `deck` (`id`, `user_id`, `visibility`, `title`) VALUES ('d2', 'u1', 'public', 'MacPub');
INSERT INTO `deck` (`id`, `user_id`, `visibility`, `title`) VALUES ('d3', NULL, 'private', 'OrphanPriv');
INSERT INTO `deck` (`id`, `user_id`, `visibility`, `title`) VALUES ('d4', NULL, 'public', 'OrphanPub');
INSERT INTO `deck` (`id`, `user_id`, `visibility`, `title`) VALUES ('d5', 'u2', 'private', 'RaePriv');
"#;

async fn backend() -> (Compiled, SqliteBackend) {
    let sf = parse_file(SCHEMA, FileId(0)).expect("parse");
    let (schema, diags) = check(&sf.decls);
    assert!(
        !diags
            .iter()
            .any(|d| d.severity == based_diagnostics::Severity::Error),
        "schema should check clean: {diags:?}"
    );
    let ddl = sql::ddl(&schema, Dialect::Sqlite);
    let compiled = Compiled::from_checked(schema, sf.decls, Dialect::Sqlite);
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("DDL failed to execute: {e:?}\n{ddl}"));
    backend.execute_batch(SEED).await.expect("seed");
    (compiled, backend)
}

async fn feed(compiled: &Compiled, backend: &SqliteBackend, ctx: serde_json::Value) -> Vec<String> {
    let ids = SeqIdGen::default();
    let resp = dispatch(
        compiled,
        backend,
        "",
        &ids,
        &NoStore,
        &Guards::new(),
        None,
        "POST",
        "/q/deck_feed",
        json!({}),
        ctx,
        None,
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    resp.body
        .as_array()
        .expect("array body")
        .iter()
        .map(|r| r["title"].as_str().expect("title").to_string())
        .collect()
}

#[tokio::test]
async fn anonymous_caller_never_sees_a_private_owned_deck() {
    let (c, backend) = backend().await;
    // Empty `$ctx` → `user` binds NULL → the leaf is `user IS NULL`, not TRUE. So the caller
    // gets: every non-private deck, plus the unowned private one (`user IS NULL`) — and NONE
    // of the private decks that belong to a real user.
    let titles = feed(&c, &backend, json!({})).await;
    assert_eq!(titles, ["MacPub", "OrphanPriv", "OrphanPub"]);
    assert!(
        !titles.contains(&"MacPriv".to_string()) && !titles.contains(&"RaePriv".to_string()),
        "a private owned deck must never leak to an anonymous caller: {titles:?}"
    );
}

#[tokio::test]
async fn signed_in_caller_sees_their_own_private_deck() {
    let (c, backend) = backend().await;
    // `$ctx.user = u1` → the leaf is `user = 'u1'`: u1 sees their own private + public decks
    // and every public deck, but not u2's private deck nor the unowned private one.
    let titles = feed(&c, &backend, json!({ "user": "u1" })).await;
    assert_eq!(titles, ["MacPriv", "MacPub", "OrphanPub"]);
}
