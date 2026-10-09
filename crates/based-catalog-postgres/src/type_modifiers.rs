//! PostgreSQL 16 native typmod decoding; preserve signed numeric scales.
use based_catalog::{NativeType, TypeFamily};

pub(crate) fn apply(native: &mut NativeType, name: &str, modifier: i32) {
    if matches!(name, "time" | "timetz" | "timestamp" | "timestamptz") {
        native.timezone = Some(matches!(name, "timetz" | "timestamptz"));
        native.precision = u16::try_from(modifier).ok();
    }
    if modifier < 4 {
        return;
    }
    match native.family {
        TypeFamily::Text => native.length = u64::try_from(modifier - 4).ok(),
        TypeFamily::Decimal => {
            let value = modifier - 4;
            native.precision = u16::try_from((value >> 16) & 0xffff).ok();
            let scale = value & 0x7ff;
            native.scale = i16::try_from((scale ^ 1024) - 1024).ok();
        }
        _ => {}
    }
}
