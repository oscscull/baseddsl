#[path = "import_support/sqlite.rs"]
mod fixture;
use fixture::{report, Fixture};

#[tokio::test]
async fn import_is_checked_deterministic_read_only_and_does_not_replace_hand_edits() {
    let fixture = Fixture::create().await;
    std::fs::create_dir(fixture.root.join("src")).unwrap();
    let output = fixture.import(&[]);
    assert!(output.status.success(), "{output:?}");
    let result = report(&output);
    assert_eq!(result["status"], "imported");
    assert_eq!(result["written"].as_array().unwrap().len(), 2);
    assert_eq!(result["catalog"]["tables"].as_array().unwrap().len(), 2);
    let account = std::fs::read(fixture.root.join("models/legacyaccount.bsl")).unwrap();
    let entry = std::fs::read(fixture.root.join("models/legacyentry.bsl")).unwrap();
    let check = fixture.run(&fixture.root, &["check"]);
    assert!(check.status.success(), "{check:?}");
    fixture.unchanged();
    let repeated = fixture.import(&[]);
    assert!(!repeated.status.success());
    assert_eq!(report(&repeated)["status"], "blocked");
    assert_eq!(
        account,
        std::fs::read(fixture.root.join("models/legacyaccount.bsl")).unwrap()
    );
    assert_eq!(
        entry,
        std::fs::read(fixture.root.join("models/legacyentry.bsl")).unwrap()
    );
    let edited = b"# hand-edited source must never be regenerated\n";
    std::fs::write(fixture.root.join("models/legacyentry.bsl"), edited).unwrap();
    assert!(!fixture.import(&[]).status.success());
    assert_eq!(
        edited,
        std::fs::read(fixture.root.join("models/legacyentry.bsl"))
            .unwrap()
            .as_slice()
    );
    fixture.unchanged();
    let fresh = Fixture::create().await;
    assert!(fresh.import(&[]).status.success());
    assert_eq!(
        account,
        std::fs::read(fresh.root.join("models/legacyaccount.bsl")).unwrap()
    );
    assert_eq!(
        entry,
        std::fs::read(fresh.root.join("models/legacyentry.bsl")).unwrap()
    );
    fresh.unchanged();
}

#[tokio::test]
async fn partial_selection_and_existing_schema_errors_write_nothing() {
    let fixture = Fixture::create().await;
    let output = fixture.run(
        &fixture.root,
        &["import", "--table", "main.legacy_entry", "--json"],
    );
    assert!(!output.status.success());
    let result = report(&output);
    assert_eq!(result["status"], "blocked");
    assert!(result["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|finding| finding["code"] == "OutsideSelection"));
    assert_eq!(
        std::fs::read_dir(fixture.root.join("models"))
            .unwrap()
            .count(),
        0
    );
    std::fs::write(
        fixture.root.join("models/handwritten.bsl"),
        "LegacyAccount { id: Id }\n",
    )
    .unwrap();
    let original = std::fs::read(fixture.root.join("models/handwritten.bsl")).unwrap();
    let collision = fixture.import(&[]);
    assert!(!collision.status.success());
    assert!(report(&collision)["compiler"]
        .as_array()
        .unwrap()
        .iter()
        .any(|finding| finding["code"] == "E0100"));
    assert_eq!(
        original,
        std::fs::read(fixture.root.join("models/handwritten.bsl")).unwrap()
    );
    assert_eq!(
        std::fs::read_dir(fixture.root.join("models"))
            .unwrap()
            .count(),
        1
    );
    fixture.unchanged();
}

#[tokio::test]
async fn output_preflight_and_connection_failures_preserve_existing_files_and_redact_values() {
    let fixture = Fixture::create().await;
    std::fs::write(
        fixture.root.join("models/legacyentry.bsl"),
        "keep this hand-edited file",
    )
    .unwrap();
    let output = fixture.import(&[]);
    assert!(!output.status.success());
    assert_eq!(
        std::fs::read_to_string(fixture.root.join("models/legacyentry.bsl")).unwrap(),
        "keep this hand-edited file"
    );
    assert!(!fixture.root.join("models/legacyaccount.bsl").exists());
    let missing = fixture.import(&["--database-url", "PRIVATE_DATABASE_DO_NOT_CREATE.db"]);
    assert!(!missing.status.success());
    assert!(!String::from_utf8_lossy(&missing.stderr).contains("PRIVATE_DATABASE_DO_NOT_CREATE"));
    assert!(!fixture
        .root
        .join("PRIVATE_DATABASE_DO_NOT_CREATE.db")
        .exists());
    fixture.unchanged();
}

#[tokio::test]
async fn malformed_server_connections_never_echo_credentials_or_change_output() {
    let fixture = Fixture::create().await;
    let original = b"hand-owned model";
    std::fs::write(fixture.root.join("models/keep.bsl"), original).unwrap();
    for (dialect, scheme) in [("postgres", "postgres"), ("mariadb", "mysql")] {
        std::fs::write(
            fixture.root.join("based.toml"),
            format!("dialect = '{dialect}'\nroot = 'models'\n"),
        )
        .unwrap();
        let url = format!("{scheme}://PRIVATE_USER:PRIVATE_PASSWORD@invalid.invalid:bad/original");
        let output = fixture.import(&["--database-url", &url]);
        assert!(!output.status.success());
        let diagnostics = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!diagnostics.contains("PRIVATE_USER"));
        assert!(!diagnostics.contains("PRIVATE_PASSWORD"));
        assert_eq!(
            original,
            std::fs::read(fixture.root.join("models/keep.bsl"))
                .unwrap()
                .as_slice()
        );
        assert_eq!(
            std::fs::read_dir(fixture.root.join("models"))
                .unwrap()
                .count(),
            1
        );
        fixture.unchanged();
    }
}
