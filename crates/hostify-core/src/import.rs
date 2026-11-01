use serde::Serialize;
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use thiserror::Error;

const MAX_FILES: u64 = 50_000;
const MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_RATIO: u64 = 200;
#[derive(Debug, Error)]
pub enum ImportError {
    #[error("Invalid source: {0}")]
    Invalid(String),
    #[error("Could not read or import source: {0}")]
    Io(#[from] io::Error),
    #[error("Invalid ZIP archive: {0}")]
    Zip(#[from] zip::result::ZipError),
}
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceReport {
    pub ok: bool,
    pub file_count: u64,
    pub size_bytes: u64,
    pub has_index: bool,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}
pub(crate) fn safe_name(name: &str) -> Result<PathBuf, ImportError> {
    if name.encode_utf16().count() > 240 {
        return Err(ImportError::Invalid("Import path is too long".into()));
    }
    if name.is_empty() || name.starts_with(['/', '\\']) || name.contains(':') {
        return Err(ImportError::Invalid(format!("Unsafe path: {name}")));
    }
    let mut result = PathBuf::new();
    for component in name.split(['/', '\\']) {
        if component.is_empty() {
            continue;
        }
        let upper = component
            .split('.')
            .next()
            .unwrap_or("")
            .to_ascii_uppercase();
        let reserved = matches!(
            upper.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        ) || (upper.len() == 4
            && (upper.starts_with("COM") || upper.starts_with("LPT"))
            && matches!(upper.as_bytes()[3], b'1'..=b'9'));
        if ["COM?", "COM?", "COM?", "LPT?", "LPT?", "LPT?"].contains(&upper.as_str())
            || component == "."
            || component == ".."
            || component.ends_with(['.', ' '])
            || reserved
            || component
                .chars()
                .any(|c| c.is_control() || matches!(c, '<' | '>' | '"' | '|' | '?' | '*'))
        {
            return Err(ImportError::Invalid(format!("Unsafe path: {name}")));
        }
        result.push(component);
    }
    if result.as_os_str().is_empty() {
        return Err(ImportError::Invalid("Empty path".into()));
    }
    Ok(result)
}
pub(crate) fn is_link(meta: &fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    false
}
/// Open the leaf itself, then validate metadata from that handle, not the scanned path.
pub(crate) fn open_regular(source: &Path, scanned: &fs::Metadata) -> Result<File, ImportError> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // OPEN_REPARSE_POINT rejects a leaf swapped to a symlink/junction; deny write/delete
        // sharing for the duration of the copy so the opened object cannot be replaced.
        options.custom_flags(0x00200000).share_mode(1);
    }
    let file = options.open(source)?;
    let opened = file.metadata()?;
    if is_link(&opened) || !opened.is_file() || opened.len() != scanned.len() {
        return Err(ImportError::Invalid(
            "Source changed or became a link while importing".into(),
        ));
    }
    Ok(file)
}
fn prune_releases(releases: &Path) -> Result<(), ImportError> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(releases)? {
        let entry = entry?;
        let meta = fs::symlink_metadata(entry.path())?;
        if is_link(&meta) || !meta.is_dir() {
            return Err(ImportError::Invalid("Unsafe release entry".into()));
        }
        let name = entry
            .file_name()
            .to_string_lossy()
            .parse::<u128>()
            .map_err(|_| ImportError::Invalid("Invalid release name".into()))?;
        entries.push((name, entry.path()));
    }
    entries.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in entries.into_iter().skip(3) {
        fs::remove_dir_all(path)?;
    }
    Ok(())
}
fn add(report: &mut SourceReport, path: &Path, size: u64) -> Result<(), ImportError> {
    report.file_count += 1;
    report.size_bytes = report
        .size_bytes
        .checked_add(size)
        .ok_or_else(|| ImportError::Invalid("Source is too large".into()))?;
    if report.file_count > MAX_FILES || report.size_bytes > MAX_BYTES {
        return Err(ImportError::Invalid("Source exceeds import limits".into()));
    }
    if path == Path::new("index.html") {
        report.has_index = true;
    }
    Ok(())
}
#[cfg(windows)]
fn open_directory(source: &Path) -> Result<File, ImportError> {
    use std::os::windows::fs::OpenOptionsExt;
    let file = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .custom_flags(0x00200000 | 0x02000000)
        .open(source)?;
    let meta = file.metadata()?;
    if is_link(&meta) || !meta.is_dir() {
        return Err(ImportError::Invalid(
            "Directory became a reparse point".into(),
        ));
    }
    Ok(file)
}
fn copy_dir(
    source: &Path,
    dest: &Path,
    relative: &Path,
    report: &mut SourceReport,
    app: bool,
) -> Result<(), ImportError> {
    if app && skip_app(relative) {
        return Ok(());
    }
    if relative
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .count()
        > 240
    {
        return Err(ImportError::Invalid("Import path is too long".into()));
    }
    let meta = fs::symlink_metadata(source)?;
    if is_link(&meta) {
        return Err(ImportError::Invalid(
            "Links and reparse points are not allowed".into(),
        ));
    }
    if meta.is_dir() {
        // Retain a non-delete-shared handle during recursion: a scanned ancestor cannot
        // be renamed to a junction while a child is being opened.
        #[cfg(windows)]
        let _directory_guard = open_directory(source)?;
        fs::create_dir_all(dest)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| ImportError::Invalid("Invalid file name".into()))?;
            safe_name(&name)?;
            copy_dir(
                &entry.path(),
                &dest.join(&name),
                &relative.join(&name),
                report,
                app,
            )?;
        }
    } else if meta.is_file() {
        let mut input = open_regular(source, &meta)?;
        add(report, relative, input.metadata()?.len())?;
        let mut output = File::create(dest)?;
        let copied = io::copy(
            &mut Read::by_ref(&mut input).take(meta.len() + 1),
            &mut output,
        )?;
        if copied != meta.len() {
            return Err(ImportError::Invalid(
                "Source changed while importing".into(),
            ));
        }
    } else {
        return Err(ImportError::Invalid(
            "Only regular files and directories are allowed".into(),
        ));
    }
    Ok(())
}
fn extract_zip(
    source: &Path,
    dest: &Path,
    report: &mut SourceReport,
    app: bool,
) -> Result<(), ImportError> {
    let mut archive = zip::ZipArchive::new(open_regular(source, &fs::symlink_metadata(source)?)?)?;
    if archive.len() as u64 > MAX_FILES * 2 {
        return Err(ImportError::Invalid("Too many archive entries".into()));
    }
    let mut entries = Vec::new();
    let mut top = HashSet::new();
    let mut root_file = false;
    for i in 0..archive.len() {
        let file = archive.by_index(i)?;
        let path = safe_name(&file.name()?)?;
        if let Some(mode) = file.unix_mode() {
            let kind = mode & 0o170000;
            if kind != 0 && kind != 0o100000 && kind != 0o040000 {
                return Err(ImportError::Invalid(
                    "Archive links or special files are not allowed".into(),
                ));
            }
        }
        if file.size() > 0
            && (file.compressed_size() == 0
                || file.size() > file.compressed_size().saturating_mul(MAX_RATIO))
        {
            return Err(ImportError::Invalid(
                "Archive compression ratio exceeds limit".into(),
            ));
        }
        let mut components = path.components();
        if let Some(first) = components.next() {
            top.insert(first.as_os_str().to_owned());
        }
        if components.next().is_none() && !file.is_dir() {
            root_file = true;
        }
        entries.push((path, file.is_dir(), file.size()));
    }
    let flatten = top.len() == 1 && !root_file;
    let mut seen = HashSet::new();
    for (i, (path, is_dir, size)) in entries.into_iter().enumerate() {
        let rel = if flatten {
            path.components().skip(1).collect::<PathBuf>()
        } else {
            path
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let key = rel.to_string_lossy().to_lowercase();
        if !seen.insert(key) {
            return Err(ImportError::Invalid("Duplicate archive path".into()));
        }
        if app && skip_app(&rel) {
            continue;
        }
        let target = dest.join(&rel);
        if is_dir {
            fs::create_dir_all(target)?;
            continue;
        }
        add(report, &rel, size)?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut input = archive.by_index(i)?;
        let mut output = File::create(target)?;
        let copied = io::copy(&mut Read::by_ref(&mut input).take(size + 1), &mut output)?;
        if copied != size {
            return Err(ImportError::Invalid("Archive entry size mismatch".into()));
        }
        output.flush()?;
    }
    Ok(())
}
fn reject_link_ancestors(path: &Path) -> Result<(), ImportError> {
    for ancestor in path.ancestors().filter(|p| !p.as_os_str().is_empty()) {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) if is_link(&meta) => {
                return Err(ImportError::Invalid(
                    "Managed destination contains a link or reparse point".into(),
                ));
            }
            Ok(_) => (),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
/// Stage within the site directory, then replace current; restores current if the swap fails.
pub fn import(source: &Path, kind: &str, site_dir: &Path) -> Result<SourceReport, ImportError> {
    reject_link_ancestors(site_dir)?;
    fs::create_dir_all(site_dir)?;
    if kind == "directory" && fs::canonicalize(site_dir)?.starts_with(fs::canonicalize(source)?) {
        return Err(ImportError::Invalid(
            "The import destination must be outside the source directory".into(),
        ));
    }
    reject_link_ancestors(&site_dir.join("current"))?;
    reject_link_ancestors(&site_dir.join("releases"))?;
    if is_link(&fs::symlink_metadata(source)?) {
        return Err(ImportError::Invalid(
            "Links and reparse points are not allowed".into(),
        ));
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ImportError::Invalid("Clock is invalid".into()))?
        .as_nanos();
    let stage = site_dir.join(format!(".staging-{}-{nonce}", std::process::id()));
    fs::create_dir(&stage)?;
    let result = (|| {
        let mut report = SourceReport::default();
        match kind {
            "directory" if source.is_dir() => {
                copy_dir(source, &stage, Path::new(""), &mut report, false)?
            }
            "zip" if source.is_file() => extract_zip(source, &stage, &mut report, false)?,
            _ => {
                return Err(ImportError::Invalid(
                    "Source does not match the selected kind".into(),
                ));
            }
        }
        if !report.has_index {
            report
                .warnings
                .push("No index.html at the site root".into());
        }
        let current = site_dir.join("current");
        let releases = site_dir.join("releases");
        fs::create_dir_all(&releases)?;
        let previous = releases.join(format!("{nonce}"));
        let had_current = current.exists();
        if had_current {
            fs::rename(&current, &previous)?;
        }
        if let Err(err) = fs::rename(&stage, &current) {
            if had_current {
                fs::rename(&previous, &current).map_err(|restore| {
                    ImportError::Invalid(format!(
                        "Swap failed: {err}; restoring previous release failed: {restore}"
                    ))
                })?;
            }
            return Err(err.into());
        }
        if let Err(e) = prune_releases(&releases) {
            report.warnings.push(format!(
                "Website files were updated, but old release cleanup needs attention: {e}"
            ));
        }
        report.ok = true;
        Ok(report)
    })();
    if stage.exists() {
        let _ = fs::remove_dir_all(&stage);
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_unsafe_names() {
        for name in [
            "../x",
            "/x",
            "C:/x",
            "\\\\server\\x",
            "a:stream",
            "a/CON.txt",
            "x/../a",
        ] {
            assert!(safe_name(name).is_err(), "{name}");
        }
    }
    #[test]
    fn imports_and_keeps_release() {
        let t = tempfile::tempdir().expect("temp");
        let src = t.path().join("src");
        fs::create_dir(&src).expect("dir");
        fs::write(src.join("index.html"), "one").expect("write");
        let site = t.path().join("site");
        assert!(import(&src, "directory", &site).expect("import").has_index);
        fs::write(src.join("index.html"), "two").expect("write");
        import(&src, "directory", &site).expect("second");
        assert_eq!(
            fs::read_to_string(site.join("current/index.html")).expect("read"),
            "two"
        );
        assert_eq!(
            fs::read_dir(site.join("releases"))
                .expect("releases")
                .count(),
            1
        );
    }
}

#[cfg(test)]
mod adversarial_tests {
    use super::*;
    use zip::{CompressionMethod, write::SimpleFileOptions};
    fn archive(name: &str, data: &[u8], mode: Option<u32>) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("a.zip");
        let f = File::create(&path).expect("file");
        let mut z = zip::ZipWriter::new(f);
        let mut options =
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        if let Some(m) = mode {
            options = options.unix_permissions(m);
        }
        z.start_file(name, options).expect("entry");
        z.write_all(data).expect("write");
        z.finish().expect("finish");
        (dir, path)
    }
    #[test]
    fn rejects_zip_slip() {
        let (dir, p) = archive("../escape", b"bad", None);
        assert!(import(&p, "zip", &dir.path().join("site")).is_err());
        assert!(!dir.path().join("escape").exists());
    }
    #[test]
    fn rejects_compression_bomb() {
        let (dir, p) = archive("index.html", &vec![0; 1_000_000], None);
        assert!(import(&p, "zip", &dir.path().join("site")).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn rejects_directory_symlink() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().expect("dir");
        let src = dir.path().join("src");
        fs::create_dir(&src).expect("mkdir");
        symlink(dir.path(), src.join("link")).expect("symlink");
        assert!(import(&src, "directory", &dir.path().join("site")).is_err());
    }
}

#[cfg(test)]
mod hardening_tests {
    use super::*;
    #[test]
    fn keeps_only_newest_three_releases() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("src");
        let site = t.path().join("site");
        fs::create_dir(&src).unwrap();
        for i in 0..7 {
            fs::write(src.join("index.html"), i.to_string()).unwrap();
            import(&src, "directory", &site).unwrap();
        }
        let mut contents = fs::read_dir(site.join("releases"))
            .unwrap()
            .map(|e| fs::read_to_string(e.unwrap().path().join("index.html")).unwrap())
            .collect::<Vec<_>>();
        contents.sort();
        assert_eq!(contents, vec!["3", "4", "5"]);
        assert_eq!(
            fs::read_to_string(site.join("current/index.html")).unwrap(),
            "6"
        );
    }
    #[test]
    fn rejects_size_change_after_scan() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("file");
        fs::write(&p, "a").unwrap();
        let scanned = fs::symlink_metadata(&p).unwrap();
        fs::write(&p, "longer").unwrap();
        assert!(open_regular(&p, &scanned).is_err());
    }
    #[cfg(windows)]
    #[test]
    fn rejects_file_symlink_swapped_after_scan() {
        use std::os::windows::fs::symlink_file;
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("file");
        let target = t.path().join("target");
        fs::write(&p, "same").unwrap();
        fs::write(&target, "same").unwrap();
        let scanned = fs::symlink_metadata(&p).unwrap();
        fs::remove_file(&p).unwrap();
        if let Err(e) = symlink_file(&target, &p) {
            eprintln!(
                "SKIP rejects_file_symlink_swapped_after_scan: Windows symlink creation unavailable: {e}"
            );
            return;
        }
        assert!(open_regular(&p, &scanned).is_err());
    }
    #[cfg(windows)]
    fn junction(link: &Path, target: &Path) -> bool {
        let result = std::process::Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(link)
            .arg(target)
            .output();
        match result {
            Ok(output) if output.status.success() => true,
            other => {
                eprintln!("SKIP junction test: Windows junction creation unavailable: {other:?}");
                false
            }
        }
    }
    #[cfg(windows)]
    #[test]
    fn rejects_directory_junction_and_destination_junction() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("src");
        let target = t.path().join("target");
        let site = t.path().join("site");
        fs::create_dir(&src).unwrap();
        fs::create_dir(&target).unwrap();
        fs::write(src.join("index.html"), "ok").unwrap();
        let link = src.join("link");
        if !junction(&link, &target) {
            return;
        }
        assert!(import(&src, "directory", &site).is_err());
        assert!(open_directory(&link).is_err());
        fs::remove_dir(&link).unwrap();
        fs::create_dir_all(&site).unwrap();
        if !junction(&site.join("releases"), &target) {
            return;
        }
        assert!(import(&src, "directory", &site).is_err());
        assert!(fs::read_dir(&target).unwrap().next().is_none());
    }
    #[cfg(windows)]
    #[test]
    fn held_source_handle_blocks_replacement_and_growth() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("file");
        fs::write(&p, "ok").unwrap();
        let f = open_regular(&p, &fs::metadata(&p).unwrap()).unwrap();
        assert!(fs::rename(&p, t.path().join("renamed")).is_err());
        assert!(fs::OpenOptions::new().write(true).open(&p).is_err());
        drop(f);
        fs::rename(&p, t.path().join("renamed")).unwrap();
        let directory = t.path().join("directory");
        fs::create_dir(&directory).unwrap();
        let guard = open_directory(&directory).unwrap();
        assert!(fs::rename(&directory, t.path().join("moved")).is_err());
        drop(guard);
        fs::rename(&directory, t.path().join("moved")).unwrap();
    }
}

#[cfg(test)]
mod audit_tests {
    use super::*;
    #[test]
    fn rejects_device_aliases_and_long_paths() {
        for name in ["CONIN$.txt", "CONOUT$", "COM?.txt", "LPT?", "LPT?.txt"] {
            assert!(safe_name(name).is_err(), "{name}");
        }
        assert!(safe_name(&"a".repeat(241)).is_err());
    }
    #[test]
    fn rejects_destination_inside_source() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("index.html"), "ok").unwrap();
        assert!(import(t.path(), "directory", &t.path().join("site")).is_err());
    }
    #[test]
    fn case_duplicate_zip_preserves_current() {
        use zip::write::SimpleFileOptions;
        let t = tempfile::tempdir().unwrap();
        let site = t.path().join("site");
        fs::create_dir_all(site.join("current")).unwrap();
        fs::write(site.join("current/index.html"), "old").unwrap();
        let path = t.path().join("source.zip");
        let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
        for name in ["index.html", "INDEX.html"] {
            zip.start_file(name, SimpleFileOptions::default()).unwrap();
            zip.write_all(b"new").unwrap();
        }
        zip.finish().unwrap();
        assert!(import(&path, "zip", &site).is_err());
        assert_eq!(
            fs::read_to_string(site.join("current/index.html")).unwrap(),
            "old"
        );
    }
    #[cfg(windows)]
    #[test]
    fn locked_current_preserves_site_and_cleans_stage() {
        use std::os::windows::fs::OpenOptionsExt;
        let t = tempfile::tempdir().unwrap();
        let source = t.path().join("source");
        let site = t.path().join("site");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("index.html"), "old").unwrap();
        import(&source, "directory", &site).unwrap();
        let _held = fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(site.join("current/index.html"))
            .unwrap();
        fs::write(source.join("index.html"), "new").unwrap();
        assert!(import(&source, "directory", &site).is_err());
        assert_eq!(
            fs::read_to_string(site.join("current/index.html")).unwrap(),
            "old"
        );
        assert!(!fs::read_dir(&site).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".staging")
        }));
    }
}

#[cfg(test)]
mod committed_swap_tests {
    use super::*;
    #[test]
    fn cleanup_failure_does_not_report_committed_update_as_failed() {
        let t = tempfile::tempdir().unwrap();
        let source = t.path().join("source");
        let site = t.path().join("site");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("index.html"), "old").unwrap();
        import(&source, "directory", &site).unwrap();
        fs::write(site.join("releases/unexpected"), "keep").unwrap();
        fs::write(source.join("index.html"), "new").unwrap();
        let report = import(&source, "directory", &site).unwrap();
        assert!(report.ok);
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("cleanup needs attention"))
        );
        assert_eq!(
            fs::read_to_string(site.join("current/index.html")).unwrap(),
            "new"
        );
    }
}

fn skip_app(path: &Path) -> bool {
    path.to_string_lossy().eq_ignore_ascii_case(".env")
        || path.components().any(|c| {
            let name = c.as_os_str().to_string_lossy();
            name.eq_ignore_ascii_case("node_modules") || name.eq_ignore_ascii_case(".git")
        })
}
/// Import an immutable application release without renaming a running working directory.
pub fn import_release(
    source: &Path,
    kind: &str,
    site_dir: &Path,
) -> Result<(SourceReport, String), ImportError> {
    reject_link_ancestors(site_dir)?;
    fs::create_dir_all(site_dir)?;
    if kind == "directory" && fs::canonicalize(site_dir)?.starts_with(fs::canonicalize(source)?) {
        return Err(ImportError::Invalid(
            "The import destination must be outside the source directory".into(),
        ));
    }
    reject_link_ancestors(&site_dir.join("app"))?;
    if is_link(&fs::symlink_metadata(source)?) {
        return Err(ImportError::Invalid(
            "Links and reparse points are not allowed".into(),
        ));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let stage = site_dir.join(format!(".staging-{id}"));
    fs::create_dir(&stage)?;
    let result = (|| {
        let mut report = SourceReport::default();
        match kind {
            "directory" if source.is_dir() => {
                copy_dir(source, &stage, Path::new(""), &mut report, true)?
            }
            "zip" if source.is_file() => extract_zip(source, &stage, &mut report, true)?,
            _ => {
                return Err(ImportError::Invalid(
                    "Source does not match the selected kind".into(),
                ));
            }
        }
        fs::create_dir_all(site_dir.join("app"))?;
        fs::rename(&stage, site_dir.join("app").join(&id))?;
        report.ok = true;
        Ok((report, id))
    })();
    if stage.exists() {
        let _ = fs::remove_dir_all(stage);
    }
    result
}
pub fn prune_app_releases(site_dir: &Path, keep: &[&str]) -> Result<(), ImportError> {
    let app = site_dir.join("app");
    reject_link_ancestors(&app)?;
    let mut entries = Vec::new();
    for e in fs::read_dir(&app)? {
        let e = e?;
        let meta = fs::symlink_metadata(e.path())?;
        if is_link(&meta)
            || !meta.is_dir()
            || uuid::Uuid::parse_str(&e.file_name().to_string_lossy()).is_err()
        {
            return Err(ImportError::Invalid(
                "Unsafe application release entry".into(),
            ));
        }
        entries.push((meta.modified()?, e));
    }
    entries.sort_by_key(|a| std::cmp::Reverse(a.0));
    for (_, e) in entries.into_iter().skip(1) {
        if !keep.contains(&e.file_name().to_string_lossy().as_ref()) {
            fs::remove_dir_all(e.path())?;
        }
    }
    Ok(())
}
#[cfg(test)]
mod app_release_tests {
    use super::*;
    #[test]
    fn releases_skip_secrets_and_dependencies_and_do_not_touch_active() {
        let source = tempfile::tempdir().unwrap();
        let managed = tempfile::tempdir().unwrap();
        for path in [
            "node_modules/pkg",
            ".git",
            "nested/node_modules/pkg",
            "nested/.git",
        ] {
            fs::create_dir_all(source.path().join(path)).unwrap();
            fs::write(source.path().join(path).join("skip"), "skip").unwrap();
        }
        fs::write(source.path().join(".env"), "PASSWORD=secret").unwrap();
        fs::create_dir_all(source.path().join("nested")).unwrap();
        fs::write(source.path().join("nested/.env"), "included").unwrap();
        fs::write(source.path().join("server.js"), "old").unwrap();
        let (_, first) = import_release(source.path(), "directory", managed.path()).unwrap();
        fs::write(source.path().join("server.js"), "new").unwrap();
        let (_, second) = import_release(source.path(), "directory", managed.path()).unwrap();
        let root = managed.path().join("app").join(&first);
        assert_eq!(fs::read_to_string(root.join("server.js")).unwrap(), "old");
        assert!(!root.join(".env").exists());
        assert!(!root.join("nested/node_modules").exists());
        assert!(!root.join(".git").exists());
        assert!(root.join("nested/.env").exists());
        let mut zip = zip::ZipWriter::new(File::create(source.path().join("input.zip")).unwrap());
        for (name, text) in [
            ("project/server.js", "zip"),
            ("project/.env", "secret"),
            ("project/node_modules/pkg/file", "skip"),
            ("project/.git/config", "skip"),
        ] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(text.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        let (_, third) =
            import_release(&source.path().join("input.zip"), "zip", managed.path()).unwrap();
        let root = managed.path().join("app").join(&third);
        assert!(!root.join(".env").exists());
        assert!(!root.join("node_modules").exists());
        prune_app_releases(managed.path(), &[&first, &second]).unwrap();
        assert!(managed.path().join("app").join(first).exists());
        assert!(root.exists());
    }
}

/// Copy configured application data before activation. Links are never followed.
pub fn copy_persistent(old: &Path, new: &Path, paths: &[String]) -> Result<(), String> {
    fn copy(source: &Path, target: &Path) -> Result<(), String> {
        let meta = fs::symlink_metadata(source).map_err(|e| e.to_string())?;
        if is_link(&meta) {
            return Err("Persistent paths cannot contain links or junctions.".into());
        }
        if let Ok(m) = fs::symlink_metadata(target) {
            if is_link(&m) {
                return Err("Persistent destination cannot contain links or junctions.".into());
            }
            if m.is_dir() != meta.is_dir() {
                return Err("Persistent path changed file type.".into());
            }
        }
        if meta.is_dir() {
            fs::create_dir_all(target).map_err(|e| e.to_string())?;
            for entry in fs::read_dir(source).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                copy(&entry.path(), &target.join(entry.file_name()))?;
            }
        } else if meta.is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut input = open_regular(source, &meta).map_err(|e| e.to_string())?;
            let mut output = File::create(target).map_err(|e| e.to_string())?;
            std::io::copy(&mut input, &mut output).map_err(|e| e.to_string())?;
        } else {
            return Err("Persistent path must be a regular file or directory.".into());
        }
        Ok(())
    }
    for text in paths {
        let rel = crate::app_config::relative(text)?;
        let source = old.join(&rel);
        reject_link_ancestors(&source).map_err(|e| e.to_string())?;
        let target = new.join(&rel);
        reject_link_ancestors(&target).map_err(|e| e.to_string())?;
        if source.exists() {
            copy(&source, &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod persistent_tests {
    use super::*;
    #[test]
    fn persistent_data_copy_and_failure_leave_active_data_unchanged() {
        let d = tempfile::tempdir().unwrap();
        let old = d.path().join("old");
        let new = d.path().join("new");
        fs::create_dir_all(old.join("uploads")).unwrap();
        fs::create_dir_all(&new).unwrap();
        fs::write(old.join("uploads/file"), "data").unwrap();
        fs::write(old.join("config.php"), "config").unwrap();
        copy_persistent(
            &old,
            &new,
            &["uploads".into(), "config.php".into(), "absent".into()],
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(new.join("uploads/file")).unwrap(),
            "data"
        );
        assert_eq!(
            fs::read_to_string(new.join("config.php")).unwrap(),
            "config"
        );
        fs::remove_file(new.join("config.php")).unwrap();
        fs::create_dir(new.join("config.php")).unwrap();
        assert!(copy_persistent(&old, &new, &["config.php".into()]).is_err());
        assert_eq!(
            fs::read_to_string(old.join("config.php")).unwrap(),
            "config"
        );
        assert!(copy_persistent(&old, &new, &["../escape".into()]).is_err());
    }
}
