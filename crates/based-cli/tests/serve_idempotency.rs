//! Durable standalone retries cross real HTTP and separate CLI processes.
#[path = "support/server.rs"]
mod server;
#[path = "support/project.rs"]
mod support;
use server::{post, Server};
use support::{success, Project};

fn project() -> Project {
    let project = Project::new();
    project.write(
        "schema/item.bsl",
        r#"
        Item { id: Id name: text owner: text }
        mutation make_item(name) -> Item { create Item { name = $name owner = $ctx.owner }; }
        query items() -> Item[] { list Item; }
    "#,
    );
    project.write(".env", "DATABASE_URL=shared.db\n");
    success(project.run("", &["migrate", "gen"]));
    success(project.run("", &["migrate", "apply"]));
    project
}

const BODY: &str = r#"{"name":"alpha"}"#;
const CTX: &str = r#"{"owner":"tenant-a"}"#;

#[test]
fn durable_retries_cross_instances_and_process_restart() {
    let project = project();
    let first = Server::start(&project, &["--init-idempotency-table"]);
    let second = Server::start(&project, &[]);
    let (a, b) = std::thread::scope(|scope| {
        let a = scope.spawn(|| post(first.address, "/m/make_item", BODY, CTX, Some("shared-key")));
        let b = scope.spawn(|| {
            post(
                second.address,
                "/m/make_item",
                BODY,
                CTX,
                Some("shared-key"),
            )
        });
        (a.join().unwrap(), b.join().unwrap())
    });
    assert_eq!(a.0, 200, "{a:?}");
    assert_eq!(b, a);
    assert_eq!(
        post(second.address, "/q/items", "{}", "{}", None)
            .1
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let changed_args = post(
        second.address,
        "/m/make_item",
        r#"{"name":"different"}"#,
        CTX,
        Some("shared-key"),
    );
    assert_eq!(changed_args.0, 422);
    let changed_context = post(
        second.address,
        "/m/make_item",
        BODY,
        r#"{"owner":"tenant-b"}"#,
        Some("shared-key"),
    );
    assert_eq!(changed_context.0, 422);
    drop(first);
    drop(second);
    let restarted = Server::start(&project, &[]);
    assert_eq!(
        post(
            restarted.address,
            "/m/make_item",
            BODY,
            CTX,
            Some("shared-key")
        ),
        a
    );
    assert_eq!(
        post(restarted.address, "/q/items", "{}", "{}", None)
            .1
            .as_array()
            .unwrap()
            .len(),
        1
    );
    for _ in 0..2 {
        assert_eq!(
            post(restarted.address, "/m/make_item", BODY, CTX, None).0,
            200
        );
    }
    assert_eq!(
        post(restarted.address, "/q/items", "{}", "{}", None)
            .1
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn missing_table_and_invalid_initialization_fail_before_listening() {
    let project = project();
    let missing = project.run("", &["serve"]);
    assert!(!missing.status.success());
    let error = String::from_utf8(missing.stderr).unwrap();
    assert!(error.contains("--init-idempotency-table"), "{error}");
    assert!(!error.contains("listening on"), "{error}");
    let invalid = project.run(
        "",
        &[
            "serve",
            "--idempotency-store",
            "memory",
            "--init-idempotency-table",
        ],
    );
    assert_eq!(invalid.status.code(), Some(2));
}

#[test]
fn memory_replays_locally_and_none_executes_each_call() {
    let project = project();
    let memory = Server::start(&project, &["--idempotency-store", "memory"]);
    let original = post(memory.address, "/m/make_item", BODY, CTX, Some("local-key"));
    assert_eq!(original.0, 200);
    assert_eq!(
        post(memory.address, "/m/make_item", BODY, CTX, Some("local-key")),
        original
    );
    drop(memory);
    let restarted = Server::start(&project, &["--idempotency-store", "memory"]);
    let new = post(
        restarted.address,
        "/m/make_item",
        BODY,
        CTX,
        Some("local-key"),
    );
    assert_eq!(new.0, 200);
    assert_ne!(new.1["id"], original.1["id"]);
    drop(restarted);
    let none = Server::start(&project, &["--idempotency-store", "none"]);
    let a = post(none.address, "/m/make_item", BODY, CTX, Some("ignored-key"));
    let b = post(none.address, "/m/make_item", BODY, CTX, Some("ignored-key"));
    assert_eq!(a.0, 200);
    assert_eq!(b.0, 200);
    assert_ne!(a.1["id"], b.1["id"]);
}
