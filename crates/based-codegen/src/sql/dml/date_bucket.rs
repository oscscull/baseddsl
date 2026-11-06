//! Lower a date-bucket call (`date(x)` / `week(x)`) to a per-dialect `DATE` expression.

use super::*;

impl<'a> Select<'a> {
    /// The bucket of the column `call` names, resolved from `alias`/`prefix` on `model`.
    /// A timestamp buckets by its UTC calendar day, the zone it is stored in.
    pub(crate) fn date_bucket(
        &mut self,
        call: &BucketCall,
        alias: &str,
        prefix: &str,
        model: &RModel,
    ) -> String {
        // Sema rejects a call without exactly one argument.
        let Some(arg) = call.arg() else {
            return "NULL".to_string();
        };
        let (a, c) = self.resolve_from(arg, alias, prefix, model);
        let col = self.qcol(&a, &c);
        let is_date = path_primitive(self.schema, model, arg) == Primitive::Date;
        date_bucket_sql(self.dialect, call.bucket, &col, is_date)
    }
}

/// Whether a bucket's result can be null: its column is nullable. Shared by the Rust
/// client and OpenAPI emitters so the two can't drift.
#[cfg(any(feature = "client", feature = "openapi"))]
pub(crate) fn bucket_nullable(
    schema: &CheckedSchema,
    model: Option<&RModel>,
    call: &BucketCall,
) -> bool {
    match (model, call.arg()) {
        (Some(m), Some(p)) => path_nullable(schema, m, p),
        _ => true,
    }
}

/// The bucket expression over `col`. A week is the Monday that starts its ISO week.
pub(crate) fn date_bucket_sql(
    dialect: Dialect,
    bucket: Bucket,
    col: &str,
    is_date: bool,
) -> String {
    match (dialect, bucket) {
        // SQLite reads the stored `…Z` text as UTC; `weekday 0` moves forward to Sunday
        // (staying put on one), so six days back lands on that week's Monday.
        (Dialect::Sqlite, Bucket::Day) => format!("date({col})"),
        (Dialect::Sqlite, Bucket::Week) => format!("date({col}, 'weekday 0', '-6 days')"),
        // A MariaDB/MySQL `DATETIME` holds the UTC wall time; `WEEKDAY` is 0 on Monday.
        (Dialect::MariaDb | Dialect::MySql, Bucket::Day) => format!("DATE({col})"),
        (Dialect::MariaDb | Dialect::MySql, Bucket::Week) => {
            format!("(DATE({col}) - INTERVAL WEEKDAY({col}) DAY)")
        }
        // A `TIMESTAMPTZ` is read at UTC so the session time zone can't shift the day.
        (Dialect::Postgres, _) => {
            let local = if is_date {
                col.to_string()
            } else {
                format!("({col} AT TIME ZONE 'UTC')")
            };
            match bucket {
                Bucket::Day => format!("({local})::date"),
                Bucket::Week => format!("date_trunc('week', ({local})::timestamp)::date"),
            }
        }
    }
}
