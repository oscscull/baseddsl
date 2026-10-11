fn main() {
    let Some(result) = based_build::run() else {
        return;
    };
    // Verification instrumentation belongs only to this fixture, not the helper.
    let record = format!("checked={} changed={}\n", result.checked, result.changed);
    std::fs::write(result.path.parent().unwrap().join("verification.txt"), record).unwrap();
}
