//! Check the runtime's actual linked engine; the dependency floor also applies to native tools.
#![cfg(feature = "sqlite")]

#[test]
fn linked_sqlite_contains_the_upstream_wal_reset_fix() {
    // SAFETY: SQLite's version functions read immutable library metadata; no handles or pointers.
    let version = unsafe { libsqlite3_sys::sqlite3_libversion_number() };
    assert!(
        version >= 3_051_003,
        "linked SQLite {version} predates the required 3.51.3 WAL-reset fix"
    );
}
