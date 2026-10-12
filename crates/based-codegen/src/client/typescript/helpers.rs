use super::*;

/// The private helpers the builders use, each emitted only when some builder needs it so
/// strict `noUnusedLocals` builds stay clean.
pub(super) fn render_helpers(fixtures: &[FixtureModel<'_>]) -> String {
    let fields = || fixtures.iter().flat_map(|f| f.fields.iter());
    let placeholder =
        |p: &Placeholder| fields().any(|f| matches!(&f.role, Role::Placeholder(q) if q == p));
    let links = |composite: bool| {
        fields().any(|f| {
            matches!(&f.value, FieldValue::Link { target, .. } if target.is_composite_key() == composite)
        })
    };
    let mut out = String::new();
    if fields().any(|f| matches!(f.role, Role::Placeholder(_))) {
        out.push_str(NEXT_N);
    }
    if [Placeholder::Timestamp, Placeholder::Date, Placeholder::Time]
        .iter()
        .any(&placeholder)
    {
        out.push_str(BASE_TIME);
    }
    for (p, helper) in [
        (Placeholder::Timestamp, TIMESTAMP),
        (Placeholder::Date, DATE),
        (Placeholder::Time, TIME),
        (Placeholder::Uuid, UUID),
        (Placeholder::Ulid, ULID),
    ] {
        if placeholder(&p) {
            out.push_str(helper);
        }
    }
    if links(false) {
        out.push_str(LINK);
    }
    if links(true) {
        out.push_str(LINK_PARTS);
    }
    out
}

const NEXT_N: &str = r#"
let counter = 1;

/** The next placeholder number: one per fixture, unique within this process. */
function nextN(): number {
  return counter++;
}
"#;

const BASE_TIME: &str = r#"
const BASE_TIME = Date.UTC(2000, 0, 1);
"#;

const TIMESTAMP: &str = r#"
/** 2000-01-01T00:00:00Z plus `n` seconds. */
function timestamp(n: number): string {
  return new Date(BASE_TIME + n * 1000).toISOString().replace(".000Z", "Z");
}
"#;

const DATE: &str = r#"
/** 2000-01-01 plus `n` days. */
function date(n: number): string {
  return new Date(BASE_TIME + n * 86400000).toISOString().slice(0, 10);
}
"#;

const TIME: &str = r#"
/** Midnight plus `n` seconds, wrapping at a day. */
function time(n: number): string {
  return new Date(BASE_TIME + (n % 86400) * 1000).toISOString().slice(11, 19);
}
"#;

const UUID: &str = r#"
function uuid(n: number): string {
  return `00000000-0000-4000-8000-${n.toString(16).padStart(12, "0")}`;
}
"#;

const ULID: &str = r#"
function ulid(n: number): string {
  return String(n).padStart(26, "0");
}
"#;

const LINK: &str = r#"
/** A relation to a single-column key: `{ <key field>: key }`. */
function link(value: unknown, keyField: string): Record<string, unknown> {
  const key = value !== null && typeof value === "object" ? (value as Record<string, unknown>)[keyField] : value;
  return { [keyField]: key };
}
"#;

const LINK_PARTS: &str = r#"
/** A relation to a composite key: just its parts. */
function linkParts(value: object, parts: readonly string[]): Record<string, unknown> {
  const fields = value as Record<string, unknown>;
  return Object.fromEntries(parts.map(part => [part, fields[part]]));
}
"#;
