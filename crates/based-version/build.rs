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
    // Only emit rerun-if-changed for paths that EXIST: cargo treats a missing declared path as
    // perpetually stale, which would re-run this script (and rebuild every downstream crate) on
    // every build. `packed-refs` is absent in a repo with only loose refs.
    if let Some(git_dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        let mut watch = vec![format!("{git_dir}/HEAD"), format!("{git_dir}/packed-refs")];
        if let Some(reference) = git(&["symbolic-ref", "-q", "HEAD"]) {
            watch.push(format!("{git_dir}/{reference}"));
        }
        for path in watch {
            if std::path::Path::new(&path).exists() {
                println!("cargo:rerun-if-changed={path}");
            }
        }
    }
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}
