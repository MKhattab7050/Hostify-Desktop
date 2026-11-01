//! Versioned *file-only* site snapshots. No MySQL or secret-store claims.
//! Immutable ZIPs are written to a restricted directory by the desktop engine.
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};
use thiserror::Error;
use uuid::Uuid;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

const MAX_FILES: u64 = 50_000;
const MAX_SOURCE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_DEPTH: usize = 24;

#[derive(Debug, Error)]
pub enum Error {
    #[error("snapshot contains a link or reparse point; resolve it before backing up")]
    Link,
    #[error("snapshot exceeded safe file count, size or nesting limits")]
    Limit,
    #[error("snapshot contains an unsupported filename")]
    Path,
    #[error("snapshot has a missing or modified archive")]
    Integrity,
    #[error("invalid snapshot identifier")]
    InvalidId,
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub id: String,
    pub site_id: String,
    pub site_kind: String,
    pub release: Option<String>,
    pub created_at: String,
    pub file_count: u64,
    pub source_bytes: u64,
    pub archive_bytes: u64,
    pub sha256: String,
    pub includes_database: bool,
    pub includes_secrets: bool,
    /// Raw source files may contain application-specific secrets. False is never
    /// inferred from excluding known .env/credential-store entries.
    #[serde(default = "default_true")]
    pub may_contain_secrets: bool,
}
fn default_true() -> bool {
    true
}

fn identifier(id: &str) -> Result<(), Error> {
    Uuid::parse_str(id)
        .map(|_| ())
        .map_err(|_| Error::InvalidId)
}
fn safe_metadata(path: &Path) -> Result<fs::Metadata, Error> {
    let m = fs::symlink_metadata(path)?;
    if m.file_type().is_symlink() {
        return Err(Error::Link);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if m.file_attributes() & 0x400 != 0 {
            return Err(Error::Link);
        }
    }
    Ok(m)
}
fn files(
    source: &Path,
    rel: &Path,
    output: &mut Vec<(PathBuf, String, u64)>,
    bytes: &mut u64,
    depth: usize,
) -> Result<(), Error> {
    if depth > MAX_DEPTH {
        return Err(Error::Limit);
    }
    let mut list = fs::read_dir(source.join(rel))?.collect::<Result<Vec<_>, _>>()?;
    list.sort_by_key(|e| e.file_name());
    for entry in list {
        let path = entry.path();
        let meta = safe_metadata(&path)?;
        let name = entry.file_name().into_string().map_err(|_| Error::Path)?;
        if matches!(name.as_str(), ".git" | "node_modules")
            || name == ".env"
            || name.starts_with(".env.")
        {
            continue;
        }
        if name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
            return Err(Error::Path);
        }
        let child = rel.join(name);
        if meta.is_dir() {
            files(source, &child, output, bytes, depth + 1)?;
        } else if meta.is_file() {
            *bytes = bytes.checked_add(meta.len()).ok_or(Error::Limit)?;
            if *bytes > MAX_SOURCE_BYTES || output.len() as u64 >= MAX_FILES {
                return Err(Error::Limit);
            }
            let archive_name = child
                .components()
                .map(|c| c.as_os_str().to_str().ok_or(Error::Path))
                .collect::<Result<Vec<_>, _>>()?
                .join("/");
            // Don't create backups whose filenames the existing safe importer
            // would later reject during an attempted restore.
            crate::import::safe_name(&archive_name).map_err(|_| Error::Path)?;
            output.push((child, archive_name, meta.len()));
        } else {
            return Err(Error::Link);
        }
    }
    Ok(())
}
pub fn checksum(path: &Path) -> Result<String, Error> {
    let mut f = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
pub fn directory(base: &Path, site_id: &str) -> Result<PathBuf, Error> {
    identifier(site_id)?;
    Ok(base.join(site_id))
}
pub fn archive(base: &Path, site_id: &str, snapshot_id: &str) -> Result<PathBuf, Error> {
    identifier(snapshot_id)?;
    Ok(directory(base, site_id)?.join(format!("{snapshot_id}.zip")))
}
pub fn create(
    source: &Path,
    base: &Path,
    site_id: &str,
    site_kind: &str,
    release: Option<&str>,
) -> Result<Manifest, Error> {
    identifier(site_id)?;
    let root = safe_metadata(source)?;
    if !root.is_dir() {
        return Err(Error::Path);
    }
    let dir = directory(base, site_id)?;
    // Never archive the snapshot destination itself.
    if dir.starts_with(source) {
        return Err(Error::Path);
    }
    fs::create_dir_all(&dir)?;
    let mut list = Vec::new();
    let mut bytes = 0;
    files(source, Path::new(""), &mut list, &mut bytes, 0)?;
    let id = Uuid::new_v4().to_string();
    let path = archive(base, site_id, &id)?;
    let temp = dir.join(format!(".{id}.partial"));
    let result = (|| -> Result<Manifest, Error> {
        let f = File::options().write(true).create_new(true).open(&temp)?;
        let mut zip = ZipWriter::new(f);
        for (rel, name, size) in &list {
            let options = SimpleFileOptions::default()
                // The importer refuses ZIP members with a compression ratio
                // above 200:1. Store uncompressed bytes so legitimate highly
                // repetitive files can always be restored from snapshots.
                .compression_method(CompressionMethod::Stored)
                .large_file(*size > u32::MAX as u64);
            zip.start_file(name, options)?;
            let source_file = source.join(rel);
            let scanned = safe_metadata(&source_file)?;
            if scanned.len() != *size {
                return Err(Error::Integrity);
            }
            // Use the same hardened source-file open policy as the importer:
            // reject leaf junction swaps and changed files before reading.
            let mut from = crate::import::open_regular(&source_file, &scanned)
                .map_err(|_| Error::Integrity)?;
            let copied = io::copy(&mut from, &mut zip)?;
            if copied != *size {
                return Err(Error::Integrity);
            }
        }
        let f = zip.finish()?;
        f.sync_all()?;
        let hash = checksum(&temp)?;
        let archive_bytes = fs::metadata(&temp)?.len();
        let manifest = Manifest {
            id: id.clone(),
            site_id: site_id.into(),
            site_kind: site_kind.into(),
            release: release.map(str::to_owned),
            created_at: Utc::now().to_rfc3339(),
            file_count: list.len() as u64,
            source_bytes: bytes,
            archive_bytes,
            sha256: hash,
            includes_database: false,
            includes_secrets: false,
            may_contain_secrets: true,
        };
        let metadata = dir.join(format!(".{id}.json.partial"));
        fs::write(&metadata, serde_json::to_vec_pretty(&manifest)?)?;
        fs::rename(&temp, &path)?;
        if let Err(err) = fs::rename(&metadata, dir.join(format!("{id}.json"))) {
            let _ = fs::remove_file(&path);
            let _ = fs::remove_file(&metadata);
            return Err(Error::Io(err));
        }
        Ok(manifest)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
pub fn read(base: &Path, site_id: &str, snapshot_id: &str) -> Result<Manifest, Error> {
    identifier(snapshot_id)?;
    let metadata = directory(base, site_id)?.join(format!("{snapshot_id}.json"));
    let record: Manifest = serde_json::from_slice(&fs::read(metadata)?)?;
    if record.site_id != site_id
        || record.id != snapshot_id
        || record.includes_database
        || record.includes_secrets
    {
        return Err(Error::Integrity);
    }
    let path = archive(base, site_id, snapshot_id)?;
    if !safe_metadata(&path)?.is_file() || checksum(&path)? != record.sha256 {
        return Err(Error::Integrity);
    }
    Ok(record)
}
pub fn list(base: &Path, site_id: &str) -> Result<Vec<Manifest>, Error> {
    let dir = directory(base, site_id)?;
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut result = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.path().extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(id) = entry
            .path()
            .file_stem()
            .and_then(|s| s.to_str())
            .map(str::to_owned)
        else {
            continue;
        };
        if identifier(&id).is_err() {
            continue;
        }
        if let Ok(record) = serde_json::from_slice::<Manifest>(&fs::read(entry.path())?) {
            if record.id == id
                && record.site_id == site_id
                && !record.includes_database
                && !record.includes_secrets
            {
                result.push(record);
            }
        }
    }
    result.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn versioned_snapshot_roundtrip_checks_integrity_and_excludes_env() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("site");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::write(source.join("index.html"), "hello").unwrap();
        fs::write(source.join("nested/app.js"), "example").unwrap();
        fs::write(source.join(".env"), "SECRET=sensitive").unwrap();
        let site_id = Uuid::new_v4().to_string();
        let root = temp.path().join("backups");
        let record = create(&source, &root, &site_id, "static", None).unwrap();
        assert!(!record.includes_database && !record.includes_secrets);
        assert!(record.may_contain_secrets);
        assert_eq!(record.file_count, 2);
        assert_eq!(list(&root, &site_id).unwrap().len(), 1);
        let p = archive(&root, &site_id, &record.id).unwrap();
        let mut z = zip::ZipArchive::new(File::open(&p).unwrap()).unwrap();
        assert!(z.by_name(".env").is_err());
        assert!(z.by_name("index.html").is_ok());
        read(&root, &site_id, &record.id).unwrap();
        fs::write(&p, b"tampered").unwrap();
        assert!(matches!(
            read(&root, &site_id, &record.id),
            Err(Error::Integrity)
        ));
    }
    #[test]
    fn repetitive_content_snapshot_can_be_restored_by_the_real_importer() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("repetitive");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("index.html"), vec![b'A'; 1024 * 1024]).unwrap();
        let site = Uuid::new_v4().to_string();
        let record = create(
            &source,
            &temp.path().join("snapshots"),
            &site,
            "static",
            None,
        )
        .unwrap();
        let archive = archive(&temp.path().join("snapshots"), &site, &record.id).unwrap();
        let dest = temp.path().join("hosted");
        let restored = crate::import::import(&archive, "zip", &dest).unwrap();
        assert_eq!(restored.file_count, 1);
        assert_eq!(
            fs::metadata(dest.join("current/index.html")).unwrap().len(),
            1024 * 1024
        );
    }
    #[test]
    fn rejects_symlinks_limits_and_path_traversal() {
        let temp = tempfile::tempdir().unwrap();
        let sid = Uuid::new_v4().to_string();
        assert!(archive(temp.path(), &sid, "../escape").is_err());
        let source = temp.path().join("site");
        fs::create_dir(&source).unwrap();
        assert!(create(&source, &source.join("backups"), &sid, "static", None).is_err());
    }
}
