//! `based explain` prints each callable's executed SQL with its bind order.
#[path = "support/project.rs"]
mod support;
use support::{success, Project};

fn project() -> Project {
    let project = Project::new();
    project.write(
        "schema/item.bsl",
        "Item { id: Id name: text status: text @index status }\n\
         query items(status?) -> Item[];\n\
         mutation rename(id: Id, name) -> Item { update Item where (id = $id) { name = $name }; }\n",
    );
    project
}

fn stdout(project: &Project, args: &[&str]) -> String {
    String::from_utf8(success(project.run("", args)).stdout).unwrap()
}

#[test]
fn explains_one_query_with_positional_binds() {
    let project = project();
    let out = stdout(&project, &["explain", ".", "items"]);
    assert!(out.starts_with("-- query items (sqlite)\n"), "{out}");
    assert!(out.contains("\n-- [1] select\nSELECT\n"), "{out}");
    assert!(out.contains("--   1  $status present (1/0)\n"), "{out}");
    assert!(out.contains("  $status\n"), "{out}");
    assert!(!out.contains(":status"), "{out}");
    assert!(!out.contains("mutation rename"), "{out}");
}

#[test]
fn a_lone_callable_argument_uses_the_discovered_project() {
    let project = project();
    assert_eq!(
        stdout(&project, &["explain", "items"]),
        stdout(&project, &["explain", ".", "items"])
    );
}

#[test]
fn explains_a_mutation_statement_by_statement() {
    let project = project();
    let out = stdout(
        &project,
        &["explain", ".", "rename", "--dialect", "postgres"],
    );
    assert!(out.starts_with("-- mutation rename (postgres)\n"), "{out}");
    assert!(out.contains("\n-- [1] "), "{out}");
    assert!(out.contains("UPDATE "), "{out}");
    assert!(out.contains("$1"), "{out}");
    assert!(
        out.contains("\n-- [2] return: re-select the declared shape\nSELECT\n"),
        "{out}"
    );
}

#[test]
fn no_callable_or_all_prints_every_callable() {
    let project = project();
    let all = stdout(&project, &["explain", ".", "--all"]);
    assert_eq!(all, stdout(&project, &["explain"]));
    let query = all.find("-- query items").unwrap();
    let mutation = all.find("-- mutation rename").unwrap();
    assert!(query < mutation, "{all}");
}

#[test]
fn unknown_callable_suggests_near_matches() {
    let project = project();
    let output = project.run("", &["explain", ".", "item"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let err = String::from_utf8(output.stderr).unwrap();
    assert!(err.contains("no query or mutation named `item`"), "{err}");
    assert!(err.contains("did you mean: items"), "{err}");
}

#[test]
fn rejects_an_unknown_dialect() {
    let project = project();
    let output = project.run("", &["explain", ".", "items", "--dialect", "oracle"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
}
