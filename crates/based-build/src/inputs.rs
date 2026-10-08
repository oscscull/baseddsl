//! Cargo discovery tracking and content fingerprints, before expensive compilation.
use crate::Error;
use based_manifest::Project;
use sha2::{Digest, Sha256};
use std::path::Path;

pub(super) struct Inputs {
    pub project: Project,
    manifest: Vec<u8>,
}
impl Inputs {
    pub(super) fn discover(root: &Path) -> Result<Self, Error> {
        let project = based_manifest::discover(root).map_err(|diagnostics| {
            based_project::Error::Diagnostics(based_project::Report {
                sources: Vec::new(),
                diagnostics,
            })
        })?;
        Ok(Self {
            project,
            manifest: std::fs::read(root.join(based_manifest::MANIFEST_NAME))?,
        })
    }

    pub(super) fn track(&self, root: &Path) {
        println!(
            "cargo:rerun-if-changed={}",
            root.join(based_manifest::MANIFEST_NAME).display()
        );
        let schema = self
            .project
            .manifest
            .root
            .as_ref()
            .map_or_else(|| root.to_path_buf(), |path| root.join(path));
        // Cargo recursively watches a directory, covering new/deleted files and directories.
        // A shared app/schema directory can rerun this cheap fingerprint step after Rust edits;
        // it cannot trigger the compiler unless BSL/manifest/generator bytes changed.
        println!("cargo:rerun-if-changed={}", schema.display());
        for file in &self.project.files {
            println!("cargo:rerun-if-changed={}", file.path.display());
        }
    }

    pub(super) fn fingerprint(&self) -> Result<String, Error> {
        let mut digest = Sha256::new();
        add(&mut digest, &self.manifest);
        for file in &self.project.files {
            add(&mut digest, file.path.as_os_str().as_encoded_bytes());
            add(&mut digest, &std::fs::read(&file.path)?);
        }
        // Include the actual host executable, so compiler/helper source changes invalidate
        // the cache even when local development retains the same package version.
        add(&mut digest, &std::fs::read(std::env::current_exe()?)?);
        Ok(format!("{:x}", digest.finalize()))
    }
}
fn add(digest: &mut Sha256, bytes: &[u8]) {
    digest.update((bytes.len() as u64).to_le_bytes());
    digest.update(bytes);
}
