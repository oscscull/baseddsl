#[path = "import_support/sqlite.rs"]
mod fixture;
use fixture::{report, Fixture};

#[tokio::test]
async fn unsupported_native_schema_reports_retained_facts_without_publishing_models() {
    let mut fixture = Fixture::create().await;
    fixture.add_schema("CREATE TABLE unsupported (key BIGINT NOT NULL PRIMARY KEY, native_value DECIMAL(10,2), computed INT GENERATED ALWAYS AS (length(native_value)) STORED, CHECK (native_value > 0));").await;
    let output = fixture.import(&["--table", "main.unsupported"]);
    assert!(!output.status.success());
    let result = report(&output);
    assert_eq!(result["status"], "blocked");
    let unsupported = result["catalog"]["tables"]
        .as_array()
        .unwrap()
        .iter()
        .find(|table| table["id"]["name"] == "unsupported")
        .unwrap();
    assert!(unsupported["native_definition"]
        .as_str()
        .unwrap()
        .contains("GENERATED ALWAYS"));
    assert!(result["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|finding| finding["code"] == "UnsupportedType"));
    assert!(result["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|finding| finding["code"] == "IncompleteMetadata"));
    assert_eq!(
        std::fs::read_dir(fixture.root.join("models"))
            .unwrap()
            .count(),
        0
    );
    fixture.unchanged();
}

#[tokio::test]
async fn output_subdirectory_is_checked_with_the_project_and_discovery_works_from_descendants() {
    let fixture = Fixture::create().await;
    std::fs::create_dir_all(fixture.root.join("src/nested")).unwrap();
    let output = fixture.run(
        &fixture.root.join("src/nested"),
        &[
            "import",
            "--table",
            "main.legacy_account",
            "--table",
            "main.legacy_entry",
            "--output",
            "models/adopted",
            "--json",
        ],
    );
    assert!(output.status.success(), "{output:?}");
    assert!(fixture
        .root
        .join("models/adopted/legacyaccount.bsl")
        .is_file());
    assert!(fixture
        .run(&fixture.root.join("src/nested"), &["check"])
        .status
        .success());
    fixture.unchanged();
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_output_is_refused_before_writing_any_model() {
    let fixture = Fixture::create().await;
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), fixture.root.join("models/linked")).unwrap();
    let output = fixture.import(&["--output", "models/linked"]);
    assert!(!output.status.success());
    assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    fixture.unchanged();
}
