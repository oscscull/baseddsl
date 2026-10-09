//! Both native numeric families must fit the existing BSL decimal precision/scale.
use based_catalog::NativeType;
pub(super) fn map(native: &NativeType) -> Option<(String, bool)> {
    let (Some(precision), Some(scale)) = (native.precision, native.scale) else {
        return None;
    };
    if !(1..=38).contains(&precision)
        || scale < 1
        || !u16::try_from(scale).is_ok_and(|scale| scale <= precision)
    {
        return None;
    }
    Some((format!("decimal({precision}, {scale})"), true))
}
