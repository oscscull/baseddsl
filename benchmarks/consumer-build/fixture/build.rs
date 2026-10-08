fn main() {
    let result = based_build::generate().unwrap_or_else(|error| panic!("{error}"));
    std::fs::write(
        result.path.parent().unwrap().join("verification.txt"),
        format!("checked={} changed={}\n", result.checked, result.changed),
    )
    .unwrap();
}
