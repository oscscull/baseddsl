//! Subprocess isolation proves dotenv precedence without changing the test runner env.
#[path = "support/project.rs"]
mod support;
use support::{success, Project};

#[test]
fn live_connection_precedence_and_selected_dotenv_are_independent_of_cwd() {
    let project = Project::new();
    project.write(
        ".env",
        "BASED_DATABASE_URL=local.db\nDATABASE_URL=unused.db\n",
    );
    success(project.run("src", &["migrate", "gen"]));
    success(project.run("src", &["migrate", "apply"]));
    let status = success(project.run("src/nested", &["migrate", "status"]));
    assert!(String::from_utf8_lossy(&status.stdout).contains("applied"));
    assert!(project.0.join("local.db").exists());
    assert!(!project.0.join("src/local.db").exists());
    assert!(!project.0.join("unused.db").exists());

    success(
        project
            .command("src", &["migrate", "status"])
            .env("DATABASE_URL", "environment.db")
            .output()
            .unwrap(),
    );
    assert!(project.0.join("environment.db").exists());
    success(
        project
            .command("src", &["migrate", "status"])
            .env("DATABASE_URL", "unused-env.db")
            .env("BASED_DATABASE_URL", "based-env.db")
            .output()
            .unwrap(),
    );
    assert!(project.0.join("based-env.db").exists());
    assert!(!project.0.join("unused-env.db").exists());
    success(
        project
            .command("src", &["migrate", "status", "--database-url", "flag.db"])
            .env("BASED_DATABASE_URL", "unused-flag.db")
            .output()
            .unwrap(),
    );
    assert!(project.0.join("flag.db").exists());
    assert!(!project.0.join("unused-flag.db").exists());

    // A distinct nested project reads only its own .env, never parent secrets.
    project.write(
        "src/based.toml",
        "dialect = \"sqlite\"\nroot = \"nested\"\n",
    );
    project.write("src/nested/model.bsl", "Child { id: Id }\n");
    let output = project.run("src/nested", &["migrate", "status"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no database url"));
}

#[test]
fn bad_dotenv_diagnostics_redact_secrets_and_higher_precedence_still_works() {
    let project = Project::new();
    project.write(".env", "DATABASE_URL='sensitive-not-for-logs\n");
    let output = project.run("", &["migrate", "status"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid project config"));
    assert!(!stderr.contains("sensitive-not-for-logs"));
    success(project.run("", &["migrate", "status", "--database-url", ":memory:"]));
    success(
        project
            .command("", &["migrate", "status"])
            .env("DATABASE_URL", ":memory:")
            .output()
            .unwrap(),
    );
    project.write(".env", "DATABASE_URL=\n");
    assert!(!project.run("", &["migrate", "status"]).status.success());
}

#[test]
fn invalid_connection_and_pool_options_fail_without_exposing_credentials() {
    let project = Project::new();
    project.write(
        ".env",
        "DATABASE_URL=postgres://user:sensitive@localhost/db\n",
    );
    let output = project.run("", &["migrate", "status"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("sqlite connection must be a file path"));
    assert!(!stderr.contains("sensitive"));
    for args in [["serve", "--pool-max", "0"], ["serve", "--pool-min", "33"]] {
        let output = project.run("", &args);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("invalid pool options"));
    }
}
