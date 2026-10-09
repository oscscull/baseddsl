//! Real CLI paths resolve one selected project for offline and live commands.
#[path = "support/project.rs"]
mod support;
use support::{success, Project};

#[test]
fn offline_commands_and_outputs_resolve_from_child_directories() {
    let project = Project::new();
    // Offline commands ignore even malformed local connection config.
    project.write(".env", "not = a 'valid assignment\n");
    success(project.run("src/nested", &["check"]));
    success(project.run("src/nested", &["fmt"]));
    success(project.run("src/nested", &["fmt", "--check"]));
    for target in ["sql", "client", "openapi"] {
        let root = success(project.run("", &["gen", target])).stdout;
        assert_eq!(
            root,
            success(project.run("src/nested", &["gen", target])).stdout
        );
        success(project.run("src/nested", &["gen", target, "-o", "artifact.txt"]));
        let saved = std::fs::read(project.0.join("artifact.txt")).unwrap();
        assert_eq!(
            artifact_payload(target, &root),
            artifact_payload(target, &saved)
        );
        assert!(String::from_utf8_lossy(&saved).contains("--out=artifact.txt"));
    }
    success(project.run("src/nested", &["migrate", "gen"]));
    assert!(project.0.join("migrations/0001_init/up.mig").exists());
    success(project.run("src/nested", &["migrate", "verify"]));
    assert_eq!(
        success(project.run("", &["migrate", "render"])).stdout,
        success(project.run("src/nested", &["migrate", "render"])).stdout
    );
}

/// File destinations change regeneration metadata, while the generated payload
/// remains independent of the invoking directory and output transport.
fn artifact_payload(target: &str, bytes: &[u8]) -> serde_json::Value {
    if target == "openapi" {
        let mut document: serde_json::Value = serde_json::from_slice(bytes).unwrap();
        document
            .as_object_mut()
            .unwrap()
            .remove("x-based-generated");
        return document;
    }
    serde_json::Value::String(
        std::str::from_utf8(bytes)
            .unwrap()
            .lines()
            .filter(|line| {
                !line.starts_with("// Regenerate ") && !line.starts_with("-- Regenerate ")
            })
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

#[test]
fn explicit_root_never_inherits_an_ancestor_project() {
    let project = Project::new();
    let missing = project.run("", &["check", "src"]);
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("no based.toml"));
    project.write("src/based.toml", "dialect = \"sqlite\"\n");
    project.write("src/inner.bsl", "Inner { id: Id }\n");
    success(project.run("src/nested", &["check"]));
    success(project.run("src/nested", &["check", project.0.to_str().unwrap()]));
}

#[test]
fn invalid_explicit_configuration_and_unreadable_schema_fail() {
    let project = Project::new();
    for option in [
        "dialect = \"sqllite\"",
        "client = \"typescript\"",
        "[schema]\nid = \"uid\"",
        "[schema]\nforeign_keys = \"convention\"",
        "clinet = \"rust\"",
    ] {
        project.write("based.toml", option);
        let out = project.run("src", &["check"]);
        assert!(!out.status.success(), "{option}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("invalid"));
    }
    project.write("based.toml", "dialect = \"sqlite\"\nroot = \"missing\"\n");
    let out = project.run("", &["check"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot read schema"));
    project.write(
        "based.toml",
        "dialect = \"sqlite\"\nroot = \"src/nested\"\n",
    );
    assert!(
        String::from_utf8_lossy(&project.run("", &["check"]).stderr).contains("no `.bsl` files")
    );
    project.write("based.toml", "dialect = \"sqlite\"\nroot = \"schema\"\n");
    assert!(!project
        .run("", &["migrate", "render", "--dialect", "sqllite"])
        .status
        .success());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let source = project.0.join("schema/item.bsl");
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o000)).unwrap();
        let output = project.run("", &["check"]);
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("reading"));
    }
}
