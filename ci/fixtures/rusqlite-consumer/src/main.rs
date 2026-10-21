//! An app that already uses rusqlite adds the engine on the same SQLite library: the
//! engine's backend and the app's own connection share one database file.

use based_runtime::{fetch_all, AdoptedRusqlite, Backend, DbRead, RusqliteBackend, SqlValue};
use rusqlite::Connection;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let path = std::env::temp_dir().join(format!("rusqlite-consumer-{}.db", std::process::id()));
    let path = path.to_str().expect("utf-8 temp path");
    let mut app = Connection::open(path).expect("app connection");
    app.execute_batch("CREATE TABLE note (id TEXT PRIMARY KEY, body TEXT NOT NULL)")
        .expect("app schema");

    let backend = RusqliteBackend::open(path).expect("engine backend");
    let mut db = backend.checkout("").await.expect("checkout");
    db.execute(
        "INSERT INTO note (id, body) VALUES (?, ?)",
        &[
            SqlValue::Text("n1".into()),
            SqlValue::Text("from the engine".into()),
        ],
    )
    .await
    .expect("engine write");
    drop(db);

    let body: String = app
        .query_row("SELECT body FROM note WHERE id = 'n1'", [], |r| r.get(0))
        .expect("app reads the engine's write");
    assert_eq!(body, "from the engine");

    app.execute_batch("BEGIN").expect("app transaction");
    let mut adopted = AdoptedRusqlite::new(&mut app);
    adopted
        .execute(
            "INSERT INTO note (id, body) VALUES (?, ?)",
            &[
                SqlValue::Text("n2".into()),
                SqlValue::Text("adopted".into()),
            ],
        )
        .await
        .expect("adopted write");
    app.execute_batch("COMMIT").expect("app commits");

    let mut db = backend.checkout("").await.expect("checkout");
    let rows = fetch_all(db.fetch("SELECT id FROM note ORDER BY id", &[]))
        .await
        .expect("engine read");
    assert_eq!(rows.len(), 2);
    println!(
        "rusqlite-consumer: one SQLite {} shared by the app and the engine",
        rusqlite::version()
    );
}
