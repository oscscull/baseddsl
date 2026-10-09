use super::metadata;
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use std::{
    fs::{self, File},
    io,
    path::{Path, PathBuf},
};

pub fn package(binary_dir: &Path, output: &Path, target: &str, source: &Value) -> Result<PathBuf> {
    let suffix = if target.contains("windows") {
        ".exe"
    } else {
        ""
    };
    fs::create_dir_all(output)?;
    let version = source["version"].as_str().unwrap();
    let artifact = output.join(format!(
        "based-{version}-{target}{}",
        if suffix.is_empty() { ".tar.gz" } else { ".zip" }
    ));
    ensure!(
        !artifact.exists(),
        "archive already exists: {}",
        artifact.display()
    );
    let scratch = tempfile::tempdir()?;
    let mut identities = serde_json::Map::new();
    for name in ["based", "based-lsp"] {
        let name = format!("{name}{suffix}");
        let binary = binary_dir.join(&name).canonicalize()?;
        identities.insert(name.clone(), json!(metadata::binary(&binary, source)?));
        fs::copy(&binary, scratch.path().join(name))?;
    }
    fs::copy(
        crate::command::root().join("LICENSE"),
        scratch.path().join("LICENSE"),
    )?;
    let notes = crate::command::root().join("book/src/upgrading.md");
    if notes.exists() {
        fs::copy(notes, scratch.path().join("RELEASE-NOTES.md"))?;
    }
    let mut manifest = source.clone();
    manifest["target"] = json!(target);
    manifest["binaries"] = json!(identities);
    fs::write(
        scratch.path().join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    fs::write(
        scratch.path().join("SOURCE.txt"),
        format!(
            "https://github.com/oscscull/baseddsl/tree/{}\n",
            source["commit"].as_str().unwrap()
        ),
    )?;
    let mut files = fs::read_dir(scratch.path())?.collect::<std::io::Result<Vec<_>>>()?;
    files.sort_by_key(|entry| entry.file_name());
    if suffix.is_empty() {
        let encoder =
            flate2::write::GzEncoder::new(File::create(&artifact)?, flate2::Compression::default());
        let mut archive = tar::Builder::new(encoder);
        for file in files {
            archive.append_path_with_name(file.path(), file.file_name())?;
        }
        archive.into_inner()?.finish()?;
        return Ok(artifact);
    }
    let mut archive = zip::ZipWriter::new(File::create(&artifact)?);
    for file in files {
        archive.start_file(
            file.file_name().to_string_lossy(),
            zip::write::SimpleFileOptions::default(),
        )?;
        io::copy(&mut File::open(file.path())?, &mut archive)?;
    }
    archive.finish()?;
    Ok(artifact)
}

pub fn extract(artifact: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    if artifact
        .extension()
        .is_some_and(|extension| extension == "zip")
    {
        let mut archive = zip::ZipArchive::new(File::open(artifact)?)?;
        for index in 0..archive.len() {
            let mut file = archive.by_index(index)?;
            let name = file.name().to_owned();
            flat(Path::new(&name))?;
            ensure!(
                file.is_file()
                    && file
                        .unix_mode()
                        .is_none_or(|mode| mode & 0o170000 != 0o120000),
                "invalid archive entry"
            );
            io::copy(&mut file, &mut File::create(destination.join(name))?)?;
        }
        return Ok(());
    }
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(File::open(artifact)?));
    for entry in archive.entries()? {
        let mut entry = entry?;
        let name = entry.path()?.into_owned();
        flat(&name)?;
        ensure!(
            entry.header().entry_type().is_file(),
            "invalid archive entry"
        );
        entry.unpack(destination.join(name))?;
    }
    Ok(())
}

fn flat(path: &Path) -> Result<()> {
    ensure!(
        path.components().count() == 1
            && matches!(
                path.components().next(),
                Some(std::path::Component::Normal(_))
            ),
        "invalid archive path"
    );
    Ok(())
}
