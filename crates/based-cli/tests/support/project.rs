use std::path::PathBuf;
use std::process::{Command, Output};

pub struct Project(pub PathBuf);

impl Project {
    pub fn new() -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("based-discovery-{}-{stamp}", std::process::id()));
        let project = Self(root);
        project.write("based.toml", "dialect = \"sqlite\"\nroot = \"schema\"\n");
        project.write(
            "schema/item.bsl",
            "Item { id: Id name: text }\nquery items() -> Item[] { list Item; }\n",
        );
        std::fs::create_dir_all(project.0.join("src/nested")).unwrap();
        project
    }

    pub fn write(&self, relative: &str, text: &str) {
        let path = self.0.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    pub fn command(&self, cwd: &str, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_based"));
        cmd.current_dir(self.0.join(cwd))
            .args(args)
            .env_remove("BASED_DATABASE_URL")
            .env_remove("DATABASE_URL")
            .env_remove("BASED_IDEMPOTENCY_STORE")
            .env_remove("BASED_INIT_IDEMPOTENCY_TABLE")
            .env_remove("BASED_GUARD_CONFIG");
        cmd
    }

    pub fn run(&self, cwd: &str, args: &[&str]) -> Output {
        self.command(cwd, args).output().unwrap()
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn success(output: Output) -> Output {
    assert!(output.status.success(), "{output:?}");
    output
}
