//! Codec-fidelity spike: the already-lowered SQL, with positional binds, run through
//! **sqlx** as a pure executor (`query` + per-value binds — no macros, no query builder)
//! against all three dialects. Each test writes every value family the runtime binds
//! (null, int, float, bool, text, uuid, timestamp, date, json, decimal, enum-checked
//! text) through sqlx into the exact column types `based gen sql` emits, reads them
//! back, and asserts the values match what the current drivers promise — exact
//! equality wherever the contract is exact (decimals above all: a decimal is its
//! wire string, every digit and trailing zero preserved).
//!
//! Infra mirrors the live suites: MariaDB/Postgres come from the Docker harnesses
//! (self-spun, or `TEST_MARIADB_URL`/`TEST_POSTGRES_URL`); no server ⇒ skip, never
//! fail. SQLite runs on a temp file via sqlx's own async SQLite driver.

#![cfg(feature = "docker-tests")]

#[path = "support/docker_mariadb.rs"]
mod docker_mariadb;
#[path = "support/docker_postgres.rs"]
mod docker_postgres;

use std::str::FromStr;

use chrono::{DateTime, NaiveDate, NaiveDateTime, Timelike, Utc};
use sqlx::types::{BigDecimal, Decimal, Uuid};
use sqlx::Row;

const UUID_1: &str = "00000000-0000-4000-8000-0000000000a1";
const UUID_2: &str = "00000000-0000-4000-8000-0000000000a2";
const UUID_3: &str = "00000000-0000-4000-8000-0000000000a3";
const UUID_4: &str = "00000000-0000-4000-8000-0000000000a4";

/// A Postgres bind carrying a value's wire text declared as the `unknown` type — the
/// current sync driver's strategy (the server coerces it like a quoted literal, so the
/// runtime never needs a parameter's column type) expressed as a sqlx `Encode`. The
/// probe below shows it does NOT survive sqlx, which transmits every parameter in
/// binary format: the server resolves the parameter to the column's type and then
/// rejects the raw text as that type's binary form.
struct WireText<'a>(&'a str);

impl sqlx::Type<sqlx::Postgres> for WireText<'_> {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        sqlx::postgres::PgTypeInfo::with_oid(sqlx::postgres::types::Oid(705)) // unknown
    }
}

impl sqlx::Encode<'_, sqlx::Postgres> for WireText<'_> {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        buf.extend_from_slice(self.0.as_bytes());
        Ok(sqlx::encode::IsNull::No)
    }
}

// Decimal fixtures at the spec's bounds (`1 ≤ s ≤ p ≤ 38`). 38 significant digits
// exceed rust_decimal's 96-bit mantissa (~28 digits), so `WIDE` is the head-to-head
// exactness probe.
const TOTAL: &str = "0.10"; // trailing zero must survive
const WIDE: &str = "12345678901234567890123456789.123456789"; // decimal(38, 9), 38 digits
const FRAC: &str = "0.00000000000000000000000000000000000001"; // decimal(38, 38), 1 at max scale
const NEG_TOTAL: &str = "-500.50";
const ZERO_FRAC: &str = "0.00000000000000000000000000000000000000"; // zero keeps its scale

const META: &str = r#"{"a":1,"tags":["x","y"]}"#;

/// The timestamp string the current MariaDB codec returns: seconds base, micros
/// appended only when nonzero.
fn mysql_ts(dt: NaiveDateTime) -> String {
    let micros = dt.nanosecond() / 1_000;
    let base = dt.format("%Y-%m-%d %H:%M:%S").to_string();
    if micros == 0 {
        base
    } else {
        format!("{base}.{micros:06}")
    }
}

/// The timestamp string the current Postgres codec returns: seconds base, micros
/// only when nonzero, `+00` suffix (the value is UTC on the wire).
fn pg_ts(dt: DateTime<Utc>) -> String {
    format!("{}+00", mysql_ts(dt.naive_utc()))
}

/// rust_decimal cannot represent 38 significant digits (96-bit mantissa, ~28 digits)
/// — and rather than refusing, its decode **silently drops** the digits beyond
/// capacity, on the Postgres binary wire and the MariaDB text wire alike. Pinning the
/// truncation is what disqualifies it as the decimal feature.
fn assert_rust_decimal_lossy(decoded: Result<Decimal, sqlx::Error>) {
    let d = decoded.expect("rust_decimal decodes an over-wide value without error");
    assert_eq!(
        d.to_string(),
        "12345678901234567890123456789",
        "rust_decimal silently truncates the fractional digits beyond its capacity"
    );
}

// ---------- MariaDB via sqlx's MySql driver --------------------------------

#[tokio::test]
async fn mariadb_values_round_trip_through_sqlx() {
    // The harness waits on the sync drivers, which block; keep that off the runtime.
    let started = tokio::task::spawn_blocking(docker_mariadb::MariaDbContainer::start)
        .await
        .expect("harness task");
    let Some(server) = started else {
        return;
    };
    let pool = sqlx::mysql::MySqlPoolOptions::new()
        .max_connections(2)
        .connect(&server.url())
        .await
        .expect("sqlx MySql driver connects to MariaDB");

    // The column types `based gen sql` emits for MariaDB, verbatim.
    sqlx::query("DROP TABLE IF EXISTS `sqlx_codec`")
        .execute(&pool)
        .await
        .expect("drop");
    sqlx::query(
        "CREATE TABLE `sqlx_codec` (\n\
           `id` UUID NOT NULL,\n\
           `name` VARCHAR(255) NULL,\n\
           `n` BIGINT NULL,\n\
           `f` DOUBLE NULL,\n\
           `ok` BOOLEAN NULL,\n\
           `at` DATETIME NULL,\n\
           `day` DATE NULL,\n\
           `meta` JSON NULL,\n\
           `total` DECIMAL(12, 2) NULL,\n\
           `wide` DECIMAL(38, 9) NULL,\n\
           `frac` DECIMAL(38, 38) NULL,\n\
           `status` VARCHAR(255) NULL,\n\
           PRIMARY KEY (`id`),\n\
           CONSTRAINT `ck_sqlx_codec_status` CHECK (`status` IN ('pending', 'paid'))\n\
         )",
    )
    .execute(&pool)
    .await
    .expect("create");

    // Bind exactly as the runtime does today: text-riding families (uuid, timestamp,
    // date, json, decimal, enum) as strings; int/bool as i64 (bool = 0/1); float as f64.
    let insert = "INSERT INTO `sqlx_codec` \
        (`id`, `name`, `n`, `f`, `ok`, `at`, `day`, `meta`, `total`, `wide`, `frac`, `status`) \
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";
    let done = sqlx::query(insert)
        .bind(UUID_1)
        .bind("Ada")
        .bind(i64::MAX)
        .bind(2.5f64)
        .bind(1i64)
        .bind("2024-01-02 12:30:45")
        .bind("2024-01-02")
        .bind(META)
        .bind(TOTAL)
        .bind(WIDE)
        .bind(FRAC)
        .bind("paid")
        .execute(&pool)
        .await
        .expect("string/i64/f64 binds coerce into every MariaDB column type");
    assert_eq!(done.rows_affected(), 1);

    // A NULL for every nullable family round-trips as NULL.
    sqlx::query("INSERT INTO `sqlx_codec` (`id`) VALUES (?)")
        .bind(UUID_2)
        .execute(&pool)
        .await
        .expect("null row");

    let row = sqlx::query("SELECT * FROM `sqlx_codec` WHERE `id` = ?")
        .bind(UUID_1)
        .fetch_one(&pool)
        .await
        .expect("select the full row back");

    // MariaDB's native UUID column returns its canonical text, but flagged with the
    // binary charset — sqlx types it BINARY and refuses a `String` decode, so the
    // codec must read raw bytes and UTF-8 them (the value IS the hyphenated string).
    let id_bytes = row.get::<Vec<u8>, _>("id");
    assert_eq!(String::from_utf8(id_bytes).expect("uuid text"), UUID_1);
    assert_eq!(row.get::<String, _>("name"), "Ada");
    assert_eq!(row.get::<i64, _>("n"), i64::MAX);
    assert_eq!(row.get::<f64, _>("f"), 2.5);
    // BOOLEAN is TINYINT(1); the wire contract is 0/1.
    assert_eq!(row.get::<bool, _>("ok") as i64, 1);
    assert_eq!(
        mysql_ts(row.get::<NaiveDateTime, _>("at")),
        "2024-01-02 12:30:45"
    );
    assert_eq!(
        row.get::<NaiveDate, _>("day")
            .format("%Y-%m-%d")
            .to_string(),
        "2024-01-02"
    );
    // MariaDB stores JSON as text and returns it exactly as inserted — but, like the
    // UUID column, wire-flagged binary (sqlx types it BLOB): decode bytes, then UTF-8.
    assert_eq!(
        String::from_utf8(row.get::<Vec<u8>, _>("meta")).expect("json text"),
        META
    );

    // Decimals: MariaDB sends DECIMAL as text on the wire, but sqlx refuses to hand
    // that text out as a String (DECIMAL is not a string type to it) — the decode
    // goes through BigDecimal, which preserves every digit and the column scale.
    // `to_plain_string` re-renders the exact wire string; `Display` does not (it
    // E-notates small values: "1E-38").
    assert_eq!(row.get::<BigDecimal, _>("total").to_plain_string(), TOTAL);
    assert_eq!(row.get::<BigDecimal, _>("wide").to_plain_string(), WIDE);
    assert_eq!(row.get::<BigDecimal, _>("frac").to_plain_string(), FRAC);
    assert_eq!(row.get::<BigDecimal, _>("frac").to_string(), "1E-38");
    assert_rust_decimal_lossy(row.try_get::<Decimal, _>("wide"));
    assert_eq!(row.get::<String, _>("status"), "paid");

    let null_row = sqlx::query("SELECT * FROM `sqlx_codec` WHERE `id` = ?")
        .bind(UUID_2)
        .fetch_one(&pool)
        .await
        .expect("null row back");
    assert_eq!(null_row.get::<Option<String>, _>("name"), None);
    assert_eq!(null_row.get::<Option<i64>, _>("n"), None);
    assert_eq!(null_row.get::<Option<f64>, _>("f"), None);
    assert_eq!(null_row.get::<Option<bool>, _>("ok"), None);
    assert_eq!(null_row.get::<Option<NaiveDateTime>, _>("at"), None);
    assert_eq!(null_row.get::<Option<BigDecimal>, _>("total"), None);
    assert_eq!(null_row.get::<Option<Vec<u8>>, _>("meta"), None);

    // The DATETIME codec itself carries microseconds (the DDL's DATETIME(0) truncates
    // at storage; that is the column's precision, not the codec's).
    let dt = sqlx::query("SELECT CAST(? AS DATETIME(6))")
        .bind("2024-01-02 12:30:45.500000")
        .fetch_one(&pool)
        .await
        .expect("datetime(6) cast")
        .get::<NaiveDateTime, _>(0);
    assert_eq!(mysql_ts(dt), "2024-01-02 12:30:45.500000");

    // Affected-rows semantics for a same-value UPDATE: the sync `mysql` crate reports
    // *changed* rows; observe what sqlx reports so the recolor knows the difference.
    let via_sqlx = sqlx::query("UPDATE `sqlx_codec` SET `name` = ? WHERE `id` = ?")
        .bind("Ada")
        .bind(UUID_1)
        .execute(&pool)
        .await
        .expect("same-value update")
        .rows_affected();
    let url = server.url();
    let via_mysql_crate = tokio::task::spawn_blocking(move || {
        use mysql::prelude::Queryable;
        let p = mysql::Pool::new(url.as_str()).expect("mysql crate pool");
        let mut conn = p.get_conn().expect("mysql crate conn");
        conn.exec_drop(
            "UPDATE `sqlx_codec` SET `name` = ? WHERE `id` = ?",
            ("Ada", UUID_1),
        )
        .expect("same-value update via mysql crate");
        conn.affected_rows()
    })
    .await
    .expect("mysql crate task");
    assert_eq!(
        (via_mysql_crate, via_sqlx),
        (0, 1),
        "mysql crate reports changed rows (0), sqlx sets CLIENT_FOUND_ROWS and reports \
         matched rows (1) — the Postgres semantics; the engine never branches on the count"
    );
}

// ---------- Postgres --------------------------------------------------------

#[tokio::test]
async fn postgres_values_round_trip_through_sqlx() {
    // The harness waits on the sync `postgres` crate, which drives its own tokio
    // `block_on`; keep that off this test's runtime.
    let started = tokio::task::spawn_blocking(docker_postgres::PostgresContainer::start)
        .await
        .expect("harness task");
    let Some(server) = started else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&server.url())
        .await
        .expect("sqlx connects to Postgres");

    // The column types `based gen sql` emits for Postgres, verbatim.
    sqlx::query("DROP TABLE IF EXISTS \"sqlx_codec\"")
        .execute(&pool)
        .await
        .expect("drop");
    sqlx::query(
        "CREATE TABLE \"sqlx_codec\" (\n\
           \"id\" UUID NOT NULL,\n\
           \"name\" TEXT NULL,\n\
           \"n\" BIGINT NULL,\n\
           \"f\" DOUBLE PRECISION NULL,\n\
           \"ok\" BOOLEAN NULL,\n\
           \"at\" TIMESTAMPTZ NULL,\n\
           \"day\" DATE NULL,\n\
           \"meta\" JSONB NULL,\n\
           \"total\" NUMERIC(12, 2) NULL,\n\
           \"wide\" NUMERIC(38, 9) NULL,\n\
           \"frac\" NUMERIC(38, 38) NULL,\n\
           \"status\" TEXT NULL,\n\
           PRIMARY KEY (\"id\"),\n\
           CONSTRAINT \"ck_sqlx_codec_status\" CHECK (\"status\" IN ('pending', 'paid'))\n\
         )",
    )
    .execute(&pool)
    .await
    .expect("create");

    // The current driver binds uuid/timestamp/json/decimal as strings and lets the
    // server coerce (text-format parameters against inferred types). sqlx declares a
    // String parameter as `text`, so the same bind is a hard type error here — the
    // recolor must bind these families as their native types instead.
    let err = sqlx::query("INSERT INTO \"sqlx_codec\" (\"id\") VALUES ($1)")
        .bind(UUID_1)
        .execute(&pool)
        .await
        .expect_err("a text-declared bind against a uuid column is rejected");
    assert!(
        err.to_string()
            .contains("is of type uuid but expression is of type text"),
        "unexpected failure shape: {err}"
    );

    // Native-typed binds carry every family. Decimals bind as BigDecimal parsed from
    // the exact wire string.
    let insert = "INSERT INTO \"sqlx_codec\" \
        (\"id\", \"name\", \"n\", \"f\", \"ok\", \"at\", \"day\", \"meta\", \"total\", \"wide\", \"frac\", \"status\") \
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)";
    let at = DateTime::parse_from_rfc3339("2024-01-02T12:30:45.500000+00:00")
        .unwrap()
        .with_timezone(&Utc);
    let done = sqlx::query(insert)
        .bind(Uuid::parse_str(UUID_1).unwrap())
        .bind("Ada")
        .bind(i64::MAX)
        .bind(2.5f64)
        .bind(true)
        .bind(at)
        .bind(NaiveDate::from_ymd_opt(2024, 1, 2).unwrap())
        .bind(serde_json::from_str::<serde_json::Value>(META).unwrap())
        .bind(BigDecimal::from_str(TOTAL).unwrap())
        .bind(BigDecimal::from_str(WIDE).unwrap())
        .bind(BigDecimal::from_str(FRAC).unwrap())
        .bind("paid")
        .execute(&pool)
        .await
        .expect("native-typed binds fill every Postgres column type");
    assert_eq!(done.rows_affected(), 1);

    // Typed NULLs for every nullable family.
    sqlx::query(insert)
        .bind(Uuid::parse_str(UUID_2).unwrap())
        .bind(Option::<String>::None)
        .bind(Option::<i64>::None)
        .bind(Option::<f64>::None)
        .bind(Option::<bool>::None)
        .bind(Option::<DateTime<Utc>>::None)
        .bind(Option::<NaiveDate>::None)
        .bind(Option::<serde_json::Value>::None)
        .bind(Option::<BigDecimal>::None)
        .bind(Option::<BigDecimal>::None)
        .bind(Option::<BigDecimal>::None)
        .bind(Option::<String>::None)
        .execute(&pool)
        .await
        .expect("typed nulls");

    // Negative + zero-with-scale decimal edge values.
    sqlx::query("INSERT INTO \"sqlx_codec\" (\"id\", \"total\", \"frac\") VALUES ($1, $2, $3)")
        .bind(Uuid::parse_str(UUID_3).unwrap())
        .bind(BigDecimal::from_str(NEG_TOTAL).unwrap())
        .bind(BigDecimal::from_str(ZERO_FRAC).unwrap())
        .execute(&pool)
        .await
        .expect("edge decimals");

    let row = sqlx::query("SELECT * FROM \"sqlx_codec\" WHERE \"id\" = $1")
        .bind(Uuid::parse_str(UUID_1).unwrap())
        .fetch_one(&pool)
        .await
        .expect("select the full row back");

    assert_eq!(row.get::<Uuid, _>("id").to_string(), UUID_1);
    assert_eq!(row.get::<String, _>("name"), "Ada");
    assert_eq!(row.get::<i64, _>("n"), i64::MAX);
    assert_eq!(row.get::<f64, _>("f"), 2.5);
    assert!(row.get::<bool, _>("ok"));
    // Microseconds survive the timestamptz round-trip.
    assert_eq!(
        pg_ts(row.get::<DateTime<Utc>, _>("at")),
        "2024-01-02 12:30:45.500000+00"
    );
    assert_eq!(
        row.get::<NaiveDate, _>("day")
            .format("%Y-%m-%d")
            .to_string(),
        "2024-01-02"
    );
    // jsonb round-trips value-exactly (the textual form is jsonb-normalized, so the
    // comparison is structural).
    assert_eq!(
        row.get::<serde_json::Value, _>("meta"),
        serde_json::from_str::<serde_json::Value>(META).unwrap()
    );

    // Decimals, the headline. BigDecimal reconstructs the exact *value* at all 38
    // digits, but its decode takes the scale from the wire's base-10000 digit groups
    // (multiples of 4) instead of the column's display scale — so its string is not
    // the wire string ("0.10" comes back "0.1000"). String-exact reads need either
    // the raw wire bytes (the existing binary decoder's input, asserted below) or a
    // `::text` projection (asserted after).
    let total = row.get::<BigDecimal, _>("total");
    assert_eq!(total, BigDecimal::from_str(TOTAL).unwrap(), "value-exact");
    assert_eq!(
        total.to_string(),
        "0.1000",
        "sqlx BigDecimal decode is value-exact but scale-inflated (base-10000 groups)"
    );
    assert_eq!(
        row.get::<BigDecimal, _>("wide"),
        BigDecimal::from_str(WIDE).unwrap(),
        "all 38 significant digits survive"
    );
    assert_rust_decimal_lossy(row.try_get::<Decimal, _>("wide"));
    assert_eq!(row.get::<String, _>("status"), "paid");

    // Raw wire bytes are available untouched: numeric's binary layout (i16 header —
    // ndigits, weight, sign, display scale — then base-10000 digits) arrives exactly
    // as the current hand-rolled binary decoder expects, display scale included.
    let raw = row.try_get_raw("total").expect("raw numeric");
    assert!(matches!(
        raw.format(),
        sqlx::postgres::PgValueFormat::Binary
    ));
    let expected_wire: Vec<u8> = [1i16, -1, 0, 2, 1000] // 0.10: one digit group, dscale 2
        .iter()
        .flat_map(|v| v.to_be_bytes())
        .collect();
    assert_eq!(raw.as_bytes().expect("bytes"), expected_wire.as_slice());

    // `::text` renders the exact wire string server-side for every bound value.
    let texts = sqlx::query(
        "SELECT \"total\"::text, \"wide\"::text, \"frac\"::text FROM \"sqlx_codec\" WHERE \"id\" = $1",
    )
    .bind(Uuid::parse_str(UUID_1).unwrap())
    .fetch_one(&pool)
    .await
    .expect("text-cast decimals");
    assert_eq!(texts.get::<String, _>(0), TOTAL);
    assert_eq!(texts.get::<String, _>(1), WIDE);
    assert_eq!(texts.get::<String, _>(2), FRAC);

    let edge =
        sqlx::query("SELECT \"total\"::text, \"frac\"::text FROM \"sqlx_codec\" WHERE \"id\" = $1")
            .bind(Uuid::parse_str(UUID_3).unwrap())
            .fetch_one(&pool)
            .await
            .expect("edge row back");
    assert_eq!(edge.get::<String, _>(0), NEG_TOTAL);
    assert_eq!(edge.get::<String, _>(1), ZERO_FRAC);

    let null_row = sqlx::query("SELECT * FROM \"sqlx_codec\" WHERE \"id\" = $1")
        .bind(Uuid::parse_str(UUID_2).unwrap())
        .fetch_one(&pool)
        .await
        .expect("null row back");
    assert_eq!(null_row.get::<Option<String>, _>("name"), None);
    assert_eq!(null_row.get::<Option<i64>, _>("n"), None);
    assert_eq!(null_row.get::<Option<bool>, _>("ok"), None);
    assert_eq!(null_row.get::<Option<DateTime<Utc>>, _>("at"), None);
    assert_eq!(null_row.get::<Option<BigDecimal>, _>("total"), None);
    assert_eq!(null_row.get::<Option<serde_json::Value>, _>("meta"), None);

    // The `unknown`-typed wire-text bind — the current driver's way of filling
    // uuid/timestamptz/jsonb/numeric from plain strings — is rejected under sqlx's
    // all-binary parameter format. Native-typed binds (above) are the only bind path.
    let err = sqlx::query(
        "INSERT INTO \"sqlx_codec\" (\"id\", \"at\", \"meta\", \"total\") VALUES ($1, $2, $3, $4)",
    )
    .bind(WireText(UUID_4))
    .bind(WireText("2024-01-02 12:30:45.500000+00"))
    .bind(WireText(META))
    .bind(WireText(TOTAL))
    .execute(&pool)
    .await
    .expect_err("wire-text-as-unknown cannot ride sqlx's binary parameter format");
    assert!(
        err.to_string().contains("incorrect binary data format"),
        "unexpected failure shape: {err}"
    );

    // The keyset guard shape (`$n = 0` against an integer literal) accepts an i64 bind:
    // sqlx declares the parameter int8, so no width inference can reject it.
    let n = sqlx::query("SELECT count(*) FROM \"sqlx_codec\" WHERE $1 = 0 OR \"n\" > $2")
        .bind(0i64)
        .bind(1i64)
        .fetch_one(&pool)
        .await
        .expect("keyset guard shape binds i64")
        .get::<i64, _>(0);
    assert_eq!(n, 3);
}

// ---------- SQLite via sqlx's async driver ----------------------------------

#[tokio::test]
async fn sqlite_values_round_trip_through_sqlx() {
    let path = std::env::temp_dir().join(format!("based_sqlx_spike_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let opts = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true);
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .expect("sqlx sqlite driver opens a file db");

    // The column types `based gen sql` emits for SQLite, verbatim: everything
    // text-riding (uuid/timestamp/date/json/decimal) is TEXT and round-trips the
    // exact string by construction; bool is INTEGER 0/1.
    sqlx::query(
        "CREATE TABLE `sqlx_codec` (\n\
           `id` TEXT NOT NULL,\n\
           `name` TEXT NULL,\n\
           `n` INTEGER NULL,\n\
           `f` REAL NULL,\n\
           `ok` INTEGER NULL,\n\
           `at` TEXT NULL,\n\
           `day` TEXT NULL,\n\
           `meta` TEXT NULL,\n\
           `total` TEXT NULL,\n\
           `wide` TEXT NULL,\n\
           `frac` TEXT NULL,\n\
           `status` TEXT NULL,\n\
           PRIMARY KEY (`id`),\n\
           CONSTRAINT `ck_sqlx_codec_status` CHECK (`status` IN ('pending', 'paid'))\n\
         )",
    )
    .execute(&pool)
    .await
    .expect("create");

    let insert = "INSERT INTO `sqlx_codec` \
        (`id`, `name`, `n`, `f`, `ok`, `at`, `day`, `meta`, `total`, `wide`, `frac`, `status`) \
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";
    let done = sqlx::query(insert)
        .bind(UUID_1)
        .bind("Ada")
        .bind(i64::MAX)
        .bind(2.5f64)
        .bind(1i64)
        .bind("2024-01-02 12:30:45.500000")
        .bind("2024-01-02")
        .bind(META)
        .bind(TOTAL)
        .bind(WIDE)
        .bind(FRAC)
        .bind("paid")
        .execute(&pool)
        .await
        .expect("string/i64/f64 binds fill every SQLite column");
    assert_eq!(done.rows_affected(), 1);

    sqlx::query("INSERT INTO `sqlx_codec` (`id`) VALUES (?)")
        .bind(UUID_2)
        .execute(&pool)
        .await
        .expect("null row");

    let row = sqlx::query("SELECT * FROM `sqlx_codec` WHERE `id` = ?")
        .bind(UUID_1)
        .fetch_one(&pool)
        .await
        .expect("select the full row back");

    assert_eq!(row.get::<String, _>("id"), UUID_1);
    assert_eq!(row.get::<String, _>("name"), "Ada");
    assert_eq!(row.get::<i64, _>("n"), i64::MAX);
    assert_eq!(row.get::<f64, _>("f"), 2.5);
    assert_eq!(row.get::<i64, _>("ok"), 1);
    // TEXT storage returns timestamps (micros intact), dates, json, and decimals as
    // the exact strings that went in.
    assert_eq!(row.get::<String, _>("at"), "2024-01-02 12:30:45.500000");
    assert_eq!(row.get::<String, _>("day"), "2024-01-02");
    assert_eq!(row.get::<String, _>("meta"), META);
    assert_eq!(row.get::<String, _>("total"), TOTAL);
    assert_eq!(row.get::<String, _>("wide"), WIDE);
    assert_eq!(row.get::<String, _>("frac"), FRAC);
    assert_eq!(row.get::<String, _>("status"), "paid");

    let null_row = sqlx::query("SELECT * FROM `sqlx_codec` WHERE `id` = ?")
        .bind(UUID_2)
        .fetch_one(&pool)
        .await
        .expect("null row back");
    assert_eq!(null_row.get::<Option<String>, _>("name"), None);
    assert_eq!(null_row.get::<Option<i64>, _>("n"), None);
    assert_eq!(null_row.get::<Option<f64>, _>("f"), None);
    assert_eq!(null_row.get::<Option<String>, _>("total"), None);

    drop(pool);
    let _ = std::fs::remove_file(&path);
}
