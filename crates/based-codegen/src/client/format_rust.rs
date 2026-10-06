/// Compatibility helper for callers that previously formatted generated Rust.
///
/// Client emitters own their layout. Their output is ready to write and must be
/// included from an isolated artifact with `include!`, so application formatting
/// cannot rewrite it. This pure identity operation preserves the existing API
/// without invoking a consumer executable or reading its formatter configuration.
pub fn format_rust(src: &str) -> String {
    src.to_owned()
}
