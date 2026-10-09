use anyhow::{ensure, Result};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::Path,
};

pub fn digest(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let size = file.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        hash.update(&buffer[..size]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub fn write(directory: &Path) -> Result<()> {
    let mut assets = fs::read_dir(directory)?
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.file_name().unwrap() != "SHA256SUMS")
        .collect::<Vec<_>>();
    assets.sort();
    let mut inventory = File::create(directory.join("SHA256SUMS"))?;
    for asset in assets {
        writeln!(
            inventory,
            "{}  {}",
            digest(&asset)?,
            asset.file_name().unwrap().to_string_lossy()
        )?;
    }
    drop(inventory);
    for line in fs::read_to_string(directory.join("SHA256SUMS"))?.lines() {
        let (expected, name) = line
            .split_once("  ")
            .ok_or_else(|| anyhow::anyhow!("invalid checksum entry"))?;
        ensure!(
            Path::new(name).file_name().and_then(|name| name.to_str()) == Some(name),
            "invalid asset path"
        );
        ensure!(
            digest(&directory.join(name))? == expected,
            "asset checksum mismatch: {name}"
        );
    }
    Ok(())
}
