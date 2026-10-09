//! Configured CLI generation, ownership, freshness, and migration-history boundaries.
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
use tempfile::TempDir;

const CONFIG: &str = "dialect = \"sqlite\"\nroot = \"schema\"\n[generate]\nclient = \"generated/client.rs\"\nclient_mode = \"embedded\"\nsql = \"generated/schema.sql\"\nopenapi = \"generated/api.json\"\n";

fn project() -> TempDir {
    let project = TempDir::new().unwrap();
    fs::create_dir_all(project.path().join("schema/nested")).unwrap();
    fs::write(project.path().join("based.toml"), CONFIG).unwrap();
    fs::write(
        project.path().join("schema/model.bsl"),
        "Item { id: Id, name: text }\nquery item(id) -> Item;\n",
    )
    .unwrap();
    project
}

fn run(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_based"))
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap()
}

fn ok(output: Output) -> Output {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
fn one_configured_command_resolves_all_destinations_from_manifest() {
    let project = project();
    let cwd = project.path().join("schema/nested");
    ok(run(&cwd, &["gen", "all"]));
    assert!(!cwd.join("generated").exists());
    let client = fs::read_to_string(project.path().join("generated/client.rs")).unwrap();
    assert!(client.contains("pub fn embedded("));
    assert!(client.contains("based gen client --out=generated/client.rs --mode embedded"));
    let before = fs::metadata(project.path().join("generated/client.rs"))
        .unwrap()
        .modified()
        .unwrap();
    ok(run(&cwd, &["gen", "all"]));
    assert_eq!(
        fs::metadata(project.path().join("generated/client.rs"))
            .unwrap()
            .modified()
            .unwrap(),
        before
    );
    ok(run(&cwd, &["gen", "all", "--check"]));
    let value: serde_json::Value =
        serde_json::from_slice(&fs::read(project.path().join("generated/api.json")).unwrap())
            .unwrap();
    assert_eq!(value["x-based-generated"]["owner"], "based");
}

#[test]
fn cli_overrides_destination_and_client_mode_without_touching_configured_output() {
    let project = project();
    ok(run(
        project.path(),
        &[
            "gen",
            "client",
            "--mode",
            "wire",
            "-o",
            "override/client.rs",
        ],
    ));
    let client = fs::read_to_string(project.path().join("override/client.rs")).unwrap();
    assert!(!client.contains("pub fn embedded("));
    assert!(client.contains("based gen client --out=override/client.rs --mode wire"));
    assert!(!project.path().join("generated").exists());
}

#[cfg(unix)]
#[test]
fn header_command_regenerates_paths_with_hyphens_spaces_and_quotes() {
    let project = project();
    let destination = "-client with 'quote'.rs";
    ok(run(
        project.path(),
        &[
            "gen",
            "client",
            &format!("--out={destination}"),
            "--mode",
            "wire",
        ],
    ));
    let path = project.path().join(destination);
    let before = fs::read(&path).unwrap();
    let content = std::str::from_utf8(&before).unwrap();
    let command = content
        .lines()
        .nth(1)
        .unwrap()
        .strip_prefix("// Regenerate from the manifest directory: based ")
        .unwrap();
    let binary = env!("CARGO_BIN_EXE_based").replace('\'', "'\"'\"'");
    let output = Command::new("/bin/sh")
        .args(["-c", &format!("'{binary}' {command}")])
        .current_dir(project.path())
        .output()
        .unwrap();
    ok(output);
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn missing_and_stale_checks_never_create_or_replace_files() {
    let project = project();
    assert!(!run(project.path(), &["gen", "all", "--check"])
        .status
        .success());
    assert!(!project.path().join("generated").exists());
    ok(run(project.path(), &["gen", "all"]));
    let path = project.path().join("generated/client.rs");
    let before = fs::read(&path).unwrap();
    fs::write(
        project.path().join("schema/added.bsl"),
        "Extra { id: Id, label: text }\nquery extra(id) -> Extra;\n",
    )
    .unwrap();
    assert!(!run(project.path(), &["gen", "client", "--check"])
        .status
        .success());
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn handwritten_output_blocks_the_whole_set_until_explicit_force() {
    let project = project();
    fs::create_dir(project.path().join("generated")).unwrap();
    let path = project.path().join("generated/api.json");
    fs::write(&path, "handwritten").unwrap();
    let output = run(project.path(), &["gen", "all"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("user-owned"));
    assert!(!project.path().join("generated/client.rs").exists());
    assert_eq!(fs::read_to_string(path).unwrap(), "handwritten");
    ok(run(project.path(), &["gen", "all", "--force"]));
    ok(run(project.path(), &["gen", "all", "--check"]));
}

#[test]
fn invalid_source_never_replaces_any_previous_output() {
    let project = project();
    ok(run(project.path(), &["gen", "all"]));
    let path = project.path().join("generated/client.rs");
    let before = fs::read(&path).unwrap();
    fs::write(project.path().join("schema/model.bsl"), "Item { broken\n").unwrap();
    let output = run(project.path(), &["gen", "all"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("model.bsl"));
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn migration_destinations_are_protected_even_with_force_and_parent_aliases() {
    let project = project();
    fs::create_dir_all(project.path().join("migrations/0001_init")).unwrap();
    let path = project.path().join("migrations/0001_init/up.mig");
    fs::write(&path, "reviewed history").unwrap();
    let output = run(
        project.path(),
        &[
            "gen",
            "sql",
            "-o",
            "generated/../migrations/0001_init/up.mig",
            "--force",
        ],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("migration history"));
    assert_eq!(fs::read_to_string(path).unwrap(), "reviewed history");
}

#[cfg(unix)]
#[test]
fn migration_aliases_and_file_symlinks_cannot_bypass_protection() {
    let project = project();
    fs::create_dir(project.path().join("migrations")).unwrap();
    std::os::unix::fs::symlink(
        project.path().join("migrations"),
        project.path().join("alias"),
    )
    .unwrap();
    assert!(!run(
        project.path(),
        &["gen", "sql", "-o", "alias/new.sql", "--force"]
    )
    .status
    .success());
    let path = project.path().join("handwritten.rs");
    fs::write(&path, "keep").unwrap();
    std::os::unix::fs::symlink(&path, project.path().join("client.rs")).unwrap();
    assert!(!run(
        project.path(),
        &["gen", "client", "-o", "client.rs", "--force"]
    )
    .status
    .success());
    assert_eq!(fs::read_to_string(path).unwrap(), "keep");
}
