//! Startup refuses incomplete or invalid mappings without leaking callback credentials.
use std::{path::PathBuf, process::Command};

struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("based-cli-guards-{}", std::process::id()));
        std::fs::create_dir_all(path.join("schema")).unwrap();
        std::fs::write(path.join("based.toml"), "dialect = \"sqlite\"\n").unwrap();
        std::fs::write(
            path.join("schema/item.bsl"),
            r#"
            Item { id: Id, name: text }
            mutation create_item(name) -> Item guard can_create { create Item { name = $name }; }
        "#,
        )
        .unwrap();
        Self(path)
    }

    fn rejects(&self, config: Option<&str>) -> String {
        let mut command = Command::new(env!("CARGO_BIN_EXE_based"));
        command
            .args([
                "serve",
                self.0.to_str().unwrap(),
                "--database-url",
                ":memory:",
                "--idempotency-store",
                "none",
            ])
            .env("BASED_GUARD_TEST_SECRET", "never-print-this-secret");
        if let Some(config) = config {
            std::fs::write(self.0.join("guards.toml"), config).unwrap();
            command.args(["--guard-config", "guards.toml"]);
        }
        let output = command.output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        let message = String::from_utf8(output.stderr).unwrap();
        assert!(!message.contains("never-print-this-secret"));
        assert!(!message.contains("URL-secret"));
        message
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn invalid_guard_configuration_prevents_startup() {
    let project = Project::new();
    assert!(project.rejects(None).contains("can_create"));
    for config in [
        "[guards]\n",
        "[guards.can_create]\nendpoint='https://user:URL-secret@localhost'\nsecret_env='BASED_GUARD_TEST_SECRET'\n",
        "[guards.can_create]\nendpoint='http://localhost'\nsecret_env='BASED_GUARD_TEST_SECRET'\n",
        "[guards.can_create]\nendpoint='https://localhost'\nsecret_env='BASED_GUARD_MISSING_SECRET'\n",
        "[guards.can_create]\nendpoint='https://localhost'\nsecret_env='BASED_GUARD_TEST_SECRET'\ndeadline_ms=0\n",
        "[guards.can_create]\nendpoint='https://localhost'\nsecret_env='BASED_GUARD_TEST_SECRET'\ndeadline_ms=30001\n",
        "[guards.unknown]\nendpoint='https://localhost'\nsecret_env='BASED_GUARD_TEST_SECRET'\n",
        "[guards.can_create]\nendpoint='https://URL-secret'\nsecret_env='BASED_GUARD_TEST_SECRET'\nunknown=true\n",
    ] { project.rejects(Some(config)); }
}
