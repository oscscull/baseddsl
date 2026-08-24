//! Captures the git commit, dirty flag, and commit date at build time and threads them
//! into the crate via `rustc-env`, so `based-version`'s consts describe the actual build.

use std::env;
use std::process::Command;

fn main() {
    let semver = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let commit = git(&["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let short = commit.get(..12).unwrap_or(&commit);
    let dirty = git(&["status", "--porcelain"]).is_some_and(|s| !s.trim().is_empty());
    let date = git(&[
        "show",
        "-s",
        "--format=%cd",
        "--date=format:%Y-%m-%d",
        "HEAD",
    ])
    .unwrap_or_else(|| "unknown".into());

    let long = format!(
        "{semver} ({short}{} {date})",
        if dirty { "-dirty" } else { "" }
    );
    println!("cargo:rustc-env=BASED_VERSION_COMMIT={commit}");
    println!("cargo:rustc-env=BASED_VERSION_LONG={long}");

    // Rebuild when the checked-out commit moves so the embedded SHA stays current.
    if let Some(git_dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        println!("cargo:rerun-if-changed={git_dir}/HEAD");
        if let Some(reference) = git(&["symbolic-ref", "-q", "HEAD"]) {
            println!("cargo:rerun-if-changed={git_dir}/{reference}");
        }
        println!("cargo:rerun-if-changed={git_dir}/packed-refs");
    }
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}
