use serde::Serialize;
use std::{collections::HashSet, fs::{self, File}, io::{self, Read, Write}, path::{Path, PathBuf}, time::{SystemTime, UNIX_EPOCH}};
use thiserror::Error;

const MAX_FILES: u64 = 50_000;
const MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_RATIO: u64 = 200;
#[derive(Debug, Error)] pub enum ImportError {
    #[error("Invalid source: {0}")] Invalid(String),
    #[error("Could not read or import source: {0}")] Io(#[from] io::Error),
    #[error("Invalid ZIP archive: {0}")] Zip(#[from] zip::result::ZipError),
}
#[derive(Debug, Default, Serialize)] #[serde(rename_all = "camelCase")]
pub struct SourceReport { pub ok: bool, pub file_count: u64, pub size_bytes: u64, pub has_index: bool, pub warnings: Vec<String>, pub errors: Vec<String> }
fn safe_name(name: &str) -> Result<PathBuf, ImportError> {
    if name.is_empty() || name.starts_with(['/', '\\']) || name.contains(':') { return Err(ImportError::Invalid(format!("Unsafe path: {name}"))); }
    let mut result = PathBuf::new();
    for component in name.split(['/', '\\']) {
        if component.is_empty() { continue; }
        let upper = component.split('.').next().unwrap_or("").to_ascii_uppercase();
        let reserved = matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL") || (upper.len() == 4 && (upper.starts_with("COM") || upper.starts_with("LPT")) && matches!(upper.as_bytes()[3], b'1'..=b'9'));
        if component == "." || component == ".." || component.ends_with(['.', ' ']) || reserved || component.chars().any(|c| c.is_control() || matches!(c, '<'|'>'|'"'|'|'|'?'|'*')) { return Err(ImportError::Invalid(format!("Unsafe path: {name}"))); }
        result.push(component);
    }
    if result.as_os_str().is_empty() { return Err(ImportError::Invalid("Empty path".into())); }
    Ok(result)
}
fn is_link(meta: &fs::Metadata) -> bool {
    if meta.file_type().is_symlink() { return true; }
    #[cfg(windows)] { use std::os::windows::fs::MetadataExt; if meta.file_attributes() & 0x400 != 0 { return true; } }
    false
}
fn add(report: &mut SourceReport, path: &Path, size: u64) -> Result<(), ImportError> {
    report.file_count += 1; report.size_bytes = report.size_bytes.checked_add(size).ok_or_else(|| ImportError::Invalid("Source is too large".into()))?;
    if report.file_count > MAX_FILES || report.size_bytes > MAX_BYTES { return Err(ImportError::Invalid("Source exceeds import limits".into())); }
    if path == Path::new("index.html") { report.has_index = true; }
    Ok(())
}
fn copy_dir(source: &Path, dest: &Path, relative: &Path, report: &mut SourceReport) -> Result<(), ImportError> {
    let meta = fs::symlink_metadata(source)?;
    if is_link(&meta) { return Err(ImportError::Invalid("Links and reparse points are not allowed".into())); }
    if meta.is_dir() {
        fs::create_dir_all(dest)?;
        for entry in fs::read_dir(source)? { let entry = entry?; let name = entry.file_name().into_string().map_err(|_| ImportError::Invalid("Invalid file name".into()))?; safe_name(&name)?; copy_dir(&entry.path(), &dest.join(&name), &relative.join(&name), report)?; }
    } else if meta.is_file() {
        add(report, relative, meta.len())?;
        let mut input = File::open(source)?; let mut output = File::create(dest)?;
        let copied = io::copy(&mut Read::by_ref(&mut input).take(meta.len() + 1), &mut output)?;
        if copied != meta.len() { return Err(ImportError::Invalid("Source changed while importing".into())); }
    } else { return Err(ImportError::Invalid("Only regular files and directories are allowed".into())); }
    Ok(())
}
fn extract_zip(source: &Path, dest: &Path, report: &mut SourceReport) -> Result<(), ImportError> {
    let mut archive = zip::ZipArchive::new(File::open(source)?)?;
    if archive.len() as u64 > MAX_FILES * 2 { return Err(ImportError::Invalid("Too many archive entries".into())); }
    let mut entries = Vec::new(); let mut top = HashSet::new(); let mut root_file = false;
    for i in 0..archive.len() {
        let file = archive.by_index(i)?;
        let path = safe_name(file.name())?;
        if let Some(mode) = file.unix_mode() { let kind = mode & 0o170000; if kind != 0 && kind != 0o100000 && kind != 0o040000 { return Err(ImportError::Invalid("Archive links or special files are not allowed".into())); } }
        if file.size() > 0 && (file.compressed_size() == 0 || file.size() > file.compressed_size().saturating_mul(MAX_RATIO)) { return Err(ImportError::Invalid("Archive compression ratio exceeds limit".into())); }
        let mut components = path.components(); if let Some(first) = components.next() { top.insert(first.as_os_str().to_owned()); }
        if components.next().is_none() && !file.is_dir() { root_file = true; }
        entries.push((path, file.is_dir(), file.size()));
    }
    let flatten = top.len() == 1 && !root_file;
    let mut seen = HashSet::new();
    for (i, (path, is_dir, size)) in entries.into_iter().enumerate() {
        let rel = if flatten { path.components().skip(1).collect::<PathBuf>() } else { path };
        if rel.as_os_str().is_empty() { continue; }
        let key = rel.to_string_lossy().to_lowercase(); if !seen.insert(key) { return Err(ImportError::Invalid("Duplicate archive path".into())); }
        let target = dest.join(&rel);
        if is_dir { fs::create_dir_all(target)?; continue; }
        add(report, &rel, size)?;
        if let Some(parent) = target.parent() { fs::create_dir_all(parent)?; }
        let mut input = archive.by_index(i)?; let mut output = File::create(target)?;
        let copied = io::copy(&mut Read::by_ref(&mut input).take(size + 1), &mut output)?;
        if copied != size { return Err(ImportError::Invalid("Archive entry size mismatch".into())); }
        output.flush()?;
    }
    Ok(())
}
/// Stage within the site directory, then replace current; restores current if the swap fails.
pub fn import(source: &Path, kind: &str, site_dir: &Path) -> Result<SourceReport, ImportError> {
    fs::create_dir_all(site_dir)?;
    if is_link(&fs::symlink_metadata(source)?) { return Err(ImportError::Invalid("Links and reparse points are not allowed".into())); }
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| ImportError::Invalid("Clock is invalid".into()))?.as_nanos();
    let stage = site_dir.join(format!(".staging-{}-{nonce}", std::process::id())); fs::create_dir(&stage)?;
    let result = (|| {
        let mut report = SourceReport::default();
        match kind { "directory" if source.is_dir() => copy_dir(source, &stage, Path::new(""), &mut report)?, "zip" if source.is_file() => extract_zip(source, &stage, &mut report)?, _ => return Err(ImportError::Invalid("Source does not match the selected kind".into())) }
        if !report.has_index { report.warnings.push("No index.html at the site root".into()); }
        let current = site_dir.join("current"); let releases = site_dir.join("releases"); fs::create_dir_all(&releases)?;
        let previous = releases.join(format!("{nonce}")); let had_current = current.exists();
        if had_current { fs::rename(&current, &previous)?; }
        if let Err(err) = fs::rename(&stage, &current) { if had_current { fs::rename(&previous, &current).map_err(|restore| ImportError::Invalid(format!("Swap failed: {err}; restoring previous release failed: {restore}")))?; } return Err(err.into()); }
        report.ok = true; Ok(report)
    })();
    if stage.exists() { let _ = fs::remove_dir_all(&stage); }
    result
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn rejects_unsafe_names() { for name in ["../x", "/x", "C:/x", "\\\\server\\x", "a:stream", "a/CON.txt", "x/../a"] { assert!(safe_name(name).is_err(), "{name}"); } }
    #[test] fn imports_and_keeps_release() { let t = tempfile::tempdir().expect("temp"); let src = t.path().join("src"); fs::create_dir(&src).expect("dir"); fs::write(src.join("index.html"), "one").expect("write"); let site = t.path().join("site"); assert!(import(&src, "directory", &site).expect("import").has_index); fs::write(src.join("index.html"), "two").expect("write"); import(&src, "directory", &site).expect("second"); assert_eq!(fs::read_to_string(site.join("current/index.html")).expect("read"), "two"); assert_eq!(fs::read_dir(site.join("releases")).expect("releases").count(), 1); }
}

#[cfg(test)] mod adversarial_tests {
 use super::*; use zip::{write::SimpleFileOptions,CompressionMethod};
 fn archive(name:&str, data:&[u8], mode:Option<u32>) -> (tempfile::TempDir,PathBuf) { let dir=tempfile::tempdir().expect("dir"); let path=dir.path().join("a.zip"); let f=File::create(&path).expect("file"); let mut z=zip::ZipWriter::new(f); let mut options=SimpleFileOptions::default().compression_method(CompressionMethod::Deflated); if let Some(m)=mode { options=options.unix_permissions(m); } z.start_file(name,options).expect("entry"); z.write_all(data).expect("write"); z.finish().expect("finish"); (dir,path) }
 #[test] fn rejects_zip_slip() { let (dir,p)=archive("../escape",b"bad",None); assert!(import(&p,"zip",&dir.path().join("site")).is_err()); assert!(!dir.path().join("escape").exists()); }
 #[test] fn rejects_compression_bomb() { let (dir,p)=archive("index.html",&vec![0;1_000_000],None); assert!(import(&p,"zip",&dir.path().join("site")).is_err()); }
 #[cfg(unix)] #[test] fn rejects_directory_symlink() { use std::os::unix::fs::symlink; let dir=tempfile::tempdir().expect("dir"); let src=dir.path().join("src"); fs::create_dir(&src).expect("mkdir"); symlink(dir.path(),src.join("link")).expect("symlink"); assert!(import(&src,"directory",&dir.path().join("site")).is_err()); }
}
