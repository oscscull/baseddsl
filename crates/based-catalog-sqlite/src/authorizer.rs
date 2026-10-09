//! SQLite's own authorizer rejects application-row reads, including count(*) probes.
use libsqlite3_sys::{sqlite3_set_authorizer, SQLITE_DENY, SQLITE_OK, SQLITE_READ};
use sqlx::SqliteConnection;
use std::{
    ffi::{c_char, c_int, c_void, CStr},
    ptr,
};

pub(crate) async fn install(connection: &mut SqliteConnection) -> Result<(), sqlx::Error> {
    let mut handle = connection.lock_handle().await?;
    // SAFETY: SQLx holds exclusive access to the live SQLite handle. The callback is
    // static, owns no borrowed state, and remains valid through connection teardown.
    let result = unsafe {
        sqlite3_set_authorizer(
            handle.as_raw_handle().as_ptr(),
            Some(authorize),
            ptr::null_mut(),
        )
    };
    if result != SQLITE_OK {
        return Err(sqlx::Error::Protocol(
            "Cannot install metadata-only authorizer".into(),
        ));
    }
    Ok(())
}

unsafe extern "C" fn authorize(
    _: *mut c_void,
    action: c_int,
    table: *const c_char,
    _: *const c_char,
    _: *const c_char,
    _: *const c_char,
) -> c_int {
    if action != SQLITE_READ {
        return SQLITE_OK;
    }
    if table.is_null() {
        return SQLITE_DENY;
    }
    // SAFETY: SQLite documents callback arguments as NULL or valid zero-terminated
    // strings for this callback's duration. We checked NULL, borrow only here, and
    // neither call into the connection nor allocate/capture mutable callback state.
    let name = unsafe { CStr::from_ptr(table) }.to_bytes();
    match name {
        b"sqlite_schema"
        | b"sqlite_master"
        | b"sqlite_temp_schema"
        | b"sqlite_temp_master"
        | b"pragma_table_list"
        | b"pragma_table_xinfo"
        | b"pragma_index_list"
        | b"pragma_index_xinfo"
        | b"pragma_foreign_key_list" => SQLITE_OK,
        _ => SQLITE_DENY,
    }
}
