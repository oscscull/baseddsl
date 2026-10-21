use super::{SqliteLocation, SqliteLocationError};
use std::path::{Path, PathBuf};

fn file(path: &str) -> Result<SqliteLocation, SqliteLocationError> {
    Ok(SqliteLocation::File(PathBuf::from(path)))
}

#[test]
fn bare_paths_are_files() {
    assert_eq!(SqliteLocation::parse("local.db"), file("local.db"));
    assert_eq!(SqliteLocation::parse("/abs/r.db"), file("/abs/r.db"));
    assert_eq!(SqliteLocation::parse("./a:b.db"), file("./a:b.db"));
    assert_eq!(
        SqliteLocation::parse(r"C:\data\r.db"),
        file(r"C:\data\r.db")
    );
    assert_eq!(SqliteLocation::parse("a%20b.db"), file("a%20b.db"));
}

#[test]
fn sqlite_url_forms_are_files() {
    assert_eq!(SqliteLocation::parse("sqlite:r3.db"), file("r3.db"));
    assert_eq!(SqliteLocation::parse("sqlite://r3.db"), file("r3.db"));
    assert_eq!(
        SqliteLocation::parse("sqlite:///abs/r2.db"),
        file("/abs/r2.db")
    );
    assert_eq!(SqliteLocation::parse("SQLITE:r.db"), file("r.db"));
    assert_eq!(SqliteLocation::parse("sqlite:a%20b.db"), file("a b.db"));
}

#[test]
fn memory_forms() {
    for value in [":memory:", "sqlite::memory:", "sqlite://:memory:"] {
        assert_eq!(
            SqliteLocation::parse(value),
            Ok(SqliteLocation::Memory),
            "{value}"
        );
    }
}

#[test]
fn other_schemes_are_rejected_without_echoing_the_value() {
    let err = SqliteLocation::parse("postgres://user:secret@host/db").unwrap_err();
    assert_eq!(err, SqliteLocationError::ForeignScheme("postgres".into()));
    assert!(!err.to_string().contains("secret"));
    for value in ["mysql://h/db", "file:x.db", "foo:bar.db"] {
        assert!(matches!(
            SqliteLocation::parse(value),
            Err(SqliteLocationError::ForeignScheme(_))
        ));
    }
}

#[test]
fn malformed_sqlite_urls_are_rejected() {
    assert_eq!(
        SqliteLocation::parse(""),
        Err(SqliteLocationError::MissingPath)
    );
    assert_eq!(
        SqliteLocation::parse("sqlite:"),
        Err(SqliteLocationError::MissingPath)
    );
    assert_eq!(
        SqliteLocation::parse("sqlite://"),
        Err(SqliteLocationError::MissingPath)
    );
    assert_eq!(
        SqliteLocation::parse("sqlite://a.db?mode=ro"),
        Err(SqliteLocationError::QueryParameters)
    );
    assert_eq!(
        SqliteLocation::parse("sqlite:a%zz.db"),
        Err(SqliteLocationError::InvalidEscape)
    );
}

#[test]
fn anchoring_joins_only_relative_files_and_round_trips() {
    let root = Path::new("/project");
    let anchored = SqliteLocation::parse("sqlite:r.db").unwrap().anchored(root);
    assert_eq!(
        anchored,
        SqliteLocation::File(PathBuf::from("/project/r.db"))
    );
    assert_eq!(
        SqliteLocation::parse(&anchored.to_path_string()),
        Ok(anchored)
    );
    let absolute = SqliteLocation::parse("sqlite:///abs/r.db")
        .unwrap()
        .anchored(root);
    assert_eq!(absolute, SqliteLocation::File(PathBuf::from("/abs/r.db")));
    assert_eq!(
        SqliteLocation::Memory.anchored(root),
        SqliteLocation::Memory
    );
}
