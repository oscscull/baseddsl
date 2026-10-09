fn main() {
    let result = based_build::generate().unwrap_or_else(|error| panic!("{error}"));
    // Verification instrumentation belongs only to this fixture, not the helper.
    let record = format!("checked={} changed={}\n", result.checked, result.changed);
    std::fs::write(result.path.parent().unwrap().join("verification.txt"), record).unwrap();
}
