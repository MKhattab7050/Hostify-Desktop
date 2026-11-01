//! Pinned runtime acquisition. Downloads and ZIP entries are streamed to disk.
use crate::components::ProbeRunner;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::io::AsyncWriteExt;
type Result<T> = std::result::Result<T, String>;
#[derive(Clone)]
pub struct Pin {
    pub kind: &'static str,
    pub version: &'static str,
    pub urls: Vec<String>,
    pub sha256: String,
    pub executable: &'static str,
    pub download_limit: u64,
    pub installed_estimate: u64,
}
struct ManifestEntry {
    kind: &'static str,
    version: &'static str,
    urls: &'static [&'static str],
    sha256: &'static str,
    executable: &'static str,
    download_limit: u64,
    installed_estimate: u64,
}
const MIB: u64 = 1024 * 1024;
/// Written last: a runtime without it was interrupted and is never used.
const INSTALLED_MARKER: &str = ".hostify-installed";
pub fn is_pinned(kind: &str, version: &str) -> bool {
    MANIFEST
        .iter()
        .any(|p| p.kind == kind && p.version == version)
}
/// Pinned versions in descending numeric order, including the PHP package revision.
pub fn pinned_versions(kind: &str) -> Vec<&'static str> {
    let mut versions = MANIFEST
        .iter()
        .filter(|p| p.kind == kind)
        .map(|p| p.version)
        .collect::<Vec<_>>();
    versions.sort_by_cached_key(|v| {
        std::cmp::Reverse(
            v.split(['.', '-'])
                .map(|n| n.parse::<u32>().unwrap_or(0))
                .collect::<Vec<_>>(),
        )
    });
    versions
}
const MANIFEST: &[ManifestEntry] = &[
    ManifestEntry {
        kind: "node",
        version: "24.22.2",
        urls: &["https://nodejs.org/dist/v24.22.2/node-v24.22.2-win-x64.zip"],
        sha256: "ca1bff5d8ead13c9f2ea404a176db46bd430704d5b5113734049fa365e7799e2",
        executable: "node.exe",
        download_limit: 80 * MIB,
        installed_estimate: 160 * MIB,
    },
    ManifestEntry {
        kind: "node",
        version: "26.13.0",
        urls: &["https://nodejs.org/dist/v26.13.0/node-v26.13.0-win-x64.zip"],
        sha256: "dcc60d59a0746999f99064a01c020445d54d84228c691cdbf25b49ac3b59150a",
        executable: "node.exe",
        download_limit: 100 * MIB,
        installed_estimate: 200 * MIB,
    },
    ManifestEntry {
        kind: "php",
        version: "8.4.27-1",
        urls: &[
            "https://windows.php.net/downloads/releases/php-8.4.27-1-nts-Win32-vs17-x64.zip",
            "https://windows.php.net/downloads/releases/archives/php-8.4.27-1-nts-Win32-vs17-x64.zip",
        ],
        sha256: "dbae154a0a89791f36c3d48e68bae797878333e7d6cb5721e74617de59b63ab7",
        executable: "php-cgi.exe",
        download_limit: 80 * MIB,
        installed_estimate: 200 * MIB,
    },
    ManifestEntry {
        kind: "php",
        version: "8.5.12-1",
        urls: &[
            "https://windows.php.net/downloads/releases/php-8.5.12-1-nts-Win32-vs17-x64.zip",
            "https://windows.php.net/downloads/releases/archives/php-8.5.12-1-nts-Win32-vs17-x64.zip",
        ],
        sha256: "2a36a2edcb7a5b9252f638fd5e4cf84d7f6e066092654220dc0b63e5cf4e9723",
        executable: "php-cgi.exe",
        download_limit: 100 * MIB,
        installed_estimate: 240 * MIB,
    },
    ManifestEntry {
        kind: "mysql",
        version: "8.4.10",
        urls: &[
            "https://cdn.mysql.com/Downloads/MySQL-8.4/mysql-8.4.10-winx64.zip",
            "https://cdn.mysql.com/archives/mysql-8.4/mysql-8.4.10-winx64.zip",
        ],
        sha256: "3b950db31c33fb59252568c012bd9ee5fac50811e778ca7c8f1a0dc91686cd6f",
        executable: "bin/mysqld.exe",
        download_limit: 400 * MIB,
        installed_estimate: 1800 * MIB,
    },
];
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub kind: String,
    pub version: String,
    pub installed: bool,
    pub ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    pub used_by: Vec<String>,
}
type SizeCache = HashMap<PathBuf, (Instant, Option<u64>, Option<String>)>;
pub type Progress = dyn Fn(&str, u64, Option<u64>) + Send + Sync;
type FreeSpace = dyn Fn(&Path) -> Result<u64> + Send + Sync;
pub struct Runtimes {
    root: PathBuf,
    pins: Vec<Pin>,
    client: reqwest::Client,
    runner: Arc<dyn ProbeRunner>,
    free: Arc<FreeSpace>,
    locks: Vec<tokio::sync::Mutex<()>>,
    sizes: Arc<Mutex<SizeCache>>,
}
fn err(e: impl std::fmt::Display) -> String {
    format!("Runtime operation failed: {e}")
}
impl Runtimes {
    pub fn new(data: &Path) -> Result<Self> {
        let client = reqwest::Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.url().scheme() != "https" {
                    attempt.error("Runtime redirects must use HTTPS")
                } else if attempt.previous().len() >= 10 {
                    attempt.error("Too many redirects")
                } else {
                    attempt.follow()
                }
            }))
            .timeout(Duration::from_secs(1800))
            .connect_timeout(Duration::from_secs(20))
            .build()
            .map_err(err)?;
        let pins = MANIFEST
            .iter()
            .map(|p| Pin {
                kind: p.kind,
                version: p.version,
                urls: p.urls.iter().map(|s| s.to_string()).collect(),
                sha256: p.sha256.into(),
                executable: p.executable,
                download_limit: p.download_limit,
                installed_estimate: p.installed_estimate,
            })
            .collect();
        Ok(Self::with_parts(
            data,
            pins,
            client,
            Arc::new(RuntimeProbe),
            Arc::new(free_space),
        ))
    }
    fn with_parts(
        data: &Path,
        pins: Vec<Pin>,
        client: reqwest::Client,
        runner: Arc<dyn ProbeRunner>,
        free: Arc<FreeSpace>,
    ) -> Self {
        let locks = pins.iter().map(|_| tokio::sync::Mutex::new(())).collect();
        Self {
            root: data.to_owned(),
            pins,
            client,
            runner,
            free,
            locks,
            sizes: Arc::new(Mutex::new(HashMap::new())),
        }
    }
    fn index(&self, kind: &str, version: &str) -> Result<usize> {
        self.pins
            .iter()
            .position(|p| p.kind == kind && p.version == version)
            .ok_or("Unknown runtime kind or version. Choose a pinned runtime from the list.".into())
    }
    fn directory(&self, p: &Pin) -> PathBuf {
        self.root.join("runtimes").join(p.kind).join(p.version)
    }
    /// Called once at engine startup, before accepting installation requests.
    pub fn clean_partial(&self) -> Result<()> {
        let path = self.root.join("runtimes/.partial");
        if path.exists() {
            std::fs::remove_dir_all(&path).map_err(err)?;
        }
        Ok(())
    }
    pub fn resolve(&self, kind: &str, version: &str) -> Result<PathBuf> {
        let p = &self.pins[self.index(kind, version)?];
        let path = self.directory(p).join(p.executable);
        if !path.is_file() || !self.directory(p).join(INSTALLED_MARKER).is_file() {
            return Err(format!(
                "{kind} {version} is not installed. Install it from Runtimes."
            ));
        }
        Ok(path)
    }
    pub async fn install(&self, kind: &str, version: &str, progress: Arc<Progress>) -> Result<()> {
        let index = self.index(kind, version)?;
        let _guard = self.locks[index]
            .try_lock()
            .map_err(|_| "This runtime is already being installed or removed.".to_string())?;
        let p = self.pins[index].clone();
        let free = (self.free)(&self.root)?;
        let needed = p.download_limit + p.installed_estimate + 100 * MIB;
        if free < needed {
            return Err(format!(
                "Installing this runtime needs {needed} bytes of free disk space, but only {free} bytes are free."
            ));
        }
        let base = self.root.join("runtimes/.partial");
        tokio::fs::create_dir_all(&base).await.map_err(err)?;
        let zip = base.join(format!("{}-{}.zip", p.kind, p.version));
        let stage = base.join(format!("{}-{}", p.kind, p.version));
        let result=async {
            self.download(&p,&zip,&progress).await?;
            progress("extract",0,None);
            let zip2=zip.clone();let stage2=stage.clone();let pin=p.clone(); let callback=progress.clone();
            tokio::task::spawn_blocking(move ||extract(&zip2,&stage2,&pin,callback.as_ref())).await.map_err(err)??;
            progress("check",0,Some(1));
            let exe=stage.join(p.executable);let runner=self.runner.clone();let pin=p.clone();
            tokio::task::spawn_blocking(move ||check(&exe,&pin,runner.as_ref())).await.map_err(err)??;
            let final_dir=self.directory(&p);let stage2=stage.clone();let backup=base.join(format!("{}-{}.old",p.kind,p.version));
            tokio::task::spawn_blocking(move || {
                std::fs::create_dir_all(final_dir.parent().unwrap()).map_err(err)?;
                let marker=final_dir.join(INSTALLED_MARKER);
                if backup.exists(){std::fs::remove_dir_all(&backup).map_err(err)?;}
                if final_dir.exists() && rename_when_released(&final_dir,&backup).is_err() {
                    // It cannot be moved aside on this volume either, and the same pinned version replaces it.
                    std::fs::remove_dir_all(&final_dir).map_err(err)?;
                }
                if rename_when_released(&stage2,&final_dir).is_err() {
                    // Some volumes refuse directory renames for reasons unrelated to locks. Copying is
                    // safe because nothing counts as installed until the marker below is written.
                    if let Err(e)=copy_tree(&stage2,&final_dir) {
                        let _=std::fs::remove_dir_all(&final_dir);
                        if backup.exists() {let _=std::fs::rename(&backup,&final_dir);}
                        return Err(format!("Could not put the checked runtime in place: {e}. Retry the installation."));
                    }
                }
                std::fs::write(&marker, b"").map_err(err)?;
                if backup.exists() {let _=std::fs::remove_dir_all(&backup);}
                Ok::<(),String>(())
            }).await.map_err(err)??;
            progress("check",1,Some(1));Ok(())
        }.await;
        let _ = tokio::fs::remove_file(zip).await;
        let _ = tokio::fs::remove_dir_all(stage).await;
        self.sizes
            .lock()
            .map_err(|_| "Runtime size cache unavailable")?
            .remove(&self.directory(&p));
        result
    }
    async fn download(&self, p: &Pin, path: &Path, progress: &Arc<Progress>) -> Result<()> {
        for (i, url) in p.urls.iter().enumerate() {
            let mut response = match self.client.get(url).send().await {
                Ok(r) if r.status() == reqwest::StatusCode::NOT_FOUND && i + 1 < p.urls.len() => {
                    continue;
                }
                Ok(r) => r.error_for_status().map_err(|_| {
                    "The runtime publisher refused the download. Try again later.".to_string()
                })?,
                Err(_) if i + 1 < p.urls.len() => continue,
                Err(_) => {
                    return Err(
                        "Could not download the runtime. Check your connection and retry.".into(),
                    );
                }
            };
            let total = response.content_length();
            if total.is_some_and(|n| n > p.download_limit) {
                return Err("Runtime download exceeds its size limit.".into());
            }
            let mut file = tokio::fs::File::create(path).await.map_err(err)?;
            let mut hash = Sha256::new();
            let mut done = 0u64;
            let mut transport = false;
            progress("download", 0, total);
            loop {
                match response.chunk().await {
                    Ok(Some(bytes)) => {
                        done = done
                            .checked_add(bytes.len() as u64)
                            .ok_or("Runtime download is too large")?;
                        if done > p.download_limit {
                            return Err("Runtime download exceeds its size limit.".into());
                        }
                        hash.update(&bytes);
                        file.write_all(&bytes).await.map_err(err)?;
                        progress("download", done, total);
                    }
                    Ok(None) => break,
                    Err(_) => {
                        transport = true;
                        break;
                    }
                }
            }
            file.flush().await.map_err(err)?;
            file.sync_all().await.map_err(err)?;
            drop(file);
            if transport {
                if i + 1 < p.urls.len() {
                    continue;
                }
                return Err(
                    "The runtime download was interrupted or truncated. Retry the installation."
                        .into(),
                );
            }
            if total.is_some_and(|n| n != done) {
                return Err("The runtime download was truncated. Retry the installation.".into());
            }
            progress("verify", done, Some(done));
            if hex::encode(hash.finalize()) != p.sha256 {
                let _ = tokio::fs::remove_file(path).await;
                return Err(
                    "The runtime download failed its integrity check. Retry the installation."
                        .into(),
                );
            }
            return Ok(());
        }
        Err("No runtime download URL is available.".into())
    }
    pub async fn list(
        &self,
        usage: HashMap<(String, String), Vec<String>>,
    ) -> Result<Vec<RuntimeStatus>> {
        let pins = self.pins.clone();
        let root = self.root.clone();
        let runner = self.runner.clone();
        let cache = self.sizes.clone();
        tokio::task::spawn_blocking(move || {
            let mut statuses = vec![];
            for p in pins {
                let dir = root.join("runtimes").join(p.kind).join(p.version);
                let exe = dir.join(p.executable);
                let installed = exe.is_file() && dir.join(INSTALLED_MARKER).is_file();
                // The version probe starts a process and the size walks a large tree, so both
                // are cached; the UI polls this list. Install and remove clear the entry.
                let cached = cache
                    .lock()
                    .map_err(|_| "Runtime size cache unavailable")?
                    .get(&dir)
                    .filter(|(t, _, _)| t.elapsed() < Duration::from_secs(60))
                    .map(|(_, size, error)| (*size, error.clone()));
                let (size, error) = if let Some(found) = cached {
                    found
                } else {
                    let error = if installed {
                        check(&exe, &p, runner.as_ref()).err()
                    } else {
                        None
                    };
                    let size = if dir.is_dir() {
                        directory_size(&dir).ok()
                    } else {
                        None
                    };
                    cache
                        .lock()
                        .map_err(|_| "Runtime size cache unavailable")?
                        .insert(dir.clone(), (Instant::now(), size, error.clone()));
                    (size, error)
                };
                statuses.push(RuntimeStatus {
                    kind: p.kind.into(),
                    version: p.version.into(),
                    installed,
                    ready: installed && error.is_none(),
                    error,
                    size_bytes: size,
                    path: installed.then_some(exe),
                    used_by: usage
                        .get(&(p.kind.into(), p.version.into()))
                        .cloned()
                        .unwrap_or_default(),
                });
            }
            Ok(statuses)
        })
        .await
        .map_err(err)?
    }
    pub async fn remove(&self, kind: &str, version: &str, used_by: &[String]) -> Result<()> {
        let index = self.index(kind, version)?;
        let _guard = self.locks[index]
            .try_lock()
            .map_err(|_| "This runtime is already being installed or removed.")?;
        if !used_by.is_empty() {
            return Err(format!(
                "Cannot remove {kind} {version}; it is used by websites: {}.",
                used_by.join(", ")
            ));
        }
        let p = &self.pins[index];
        let dir = self.directory(p);
        // Future MySQL tasks must use one of these managed data roots, never the install tree.
        if kind == "mysql"
            && [
                self.root.join("mysql/data"),
                self.root.join("mysql").join(version).join("data"),
                self.root.join("databases/mysql"),
                dir.join("data"),
            ]
            .iter()
            .any(|p| p.exists())
        {
            return Err(format!(
                "Cannot remove MySQL while the data directory {} exists. Uninstalling preserves data.",
                self.root.join("mysql/data").display()
            ));
        }
        if dir.exists() {
            tokio::fs::remove_dir_all(&dir).await.map_err(err)?;
        }
        self.sizes
            .lock()
            .map_err(|_| "Runtime size cache unavailable")?
            .remove(&dir);
        Ok(())
    }
}
/// Runtime probes retain bounded stderr as well as stdout, including loader errors.
struct RuntimeProbe;
impl ProbeRunner for RuntimeProbe {
    fn version(&self, path: &Path, arg: &str) -> Result<String> {
        use std::process::{Command, Stdio};
        let mut command = Command::new(path);
        command
            .arg(arg)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command.spawn().map_err(err)?;
        let (sender, receiver) = std::sync::mpsc::channel();
        fn capture(input: impl Read + Send + 'static, sender: std::sync::mpsc::Sender<Vec<u8>>) {
            std::thread::spawn(move || {
                let mut bytes = Vec::new();
                let _ = input.take(16 * 1024).read_to_end(&mut bytes);
                let _ = sender.send(bytes);
            });
        }
        capture(
            child.stdout.take().ok_or("Version stdout unavailable")?,
            sender.clone(),
        );
        capture(
            child.stderr.take().ok_or("Version stderr unavailable")?,
            sender,
        );
        let start = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if start.elapsed() < Duration::from_secs(5) => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                result => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(match result {
                        Err(e) => err(e),
                        _ => "Runtime version check timed out after five seconds.".into(),
                    });
                }
            }
        };
        let mut output = String::new();
        for _ in 0..2 {
            let bytes = receiver
                .recv_timeout(Duration::from_millis(200))
                .map_err(err)?;
            output.push_str(&String::from_utf8_lossy(&bytes));
        }
        if !status.success() {
            return Err(format!(
                "Version command exited with code {}: {output}",
                status
                    .code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "unknown".into())
            ));
        }
        if output.to_ascii_lowercase().contains("vcruntime140") {
            return Err(output);
        }
        Ok(output)
    }
}
/// Antivirus scanners briefly hold newly written executables open, which makes a directory
/// rename fail with "access denied" on Windows. Wait for them rather than failing the install.
fn rename_when_released(from: &Path, to: &Path) -> std::io::Result<()> {
    let start = Instant::now();
    loop {
        match std::fs::rename(from, to) {
            Err(e)
                if e.kind() == std::io::ErrorKind::PermissionDenied
                    && start.elapsed() < Duration::from_secs(10) =>
            {
                std::thread::sleep(Duration::from_millis(500))
            }
            result => return result,
        }
    }
}
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}
fn check(exe: &Path, p: &Pin, runner: &dyn ProbeRunner) -> Result<()> {
    if !exe.is_file() {
        return Err("The runtime executable is missing. Reinstall this runtime.".into());
    }
    let output = runner
        .version(exe, if p.kind == "php" { "-v" } else { "--version" })
        .map_err(probe_error)?;
    if !output.contains(p.version.split('-').next().unwrap_or(p.version)) {
        return Err(
            "The runtime version does not match the pinned version. Reinstall this runtime.".into(),
        );
    }
    Ok(())
}
fn probe_error(message: String) -> String {
    if ["c0000135", "-1073741515", "3221225781", "vcruntime140"]
        .iter()
        .any(|s| message.to_ascii_lowercase().contains(s))
    {
        "The Microsoft Visual C++ runtime is absent. Install Microsoft's Visual C++ Redistributable for x64 from https://aka.ms/vs/17/release/vc_redist.x64.exe, then retry.".into()
    } else {
        format!("The runtime version check failed: {message}")
    }
}
fn directory_size(dir: &Path) -> std::io::Result<u64> {
    let mut size = 0u64;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_dir() {
            size = size.saturating_add(directory_size(&entry.path())?);
        } else {
            size = size.saturating_add(meta.len());
        }
    }
    Ok(size)
}
fn extract(zip: &Path, stage: &Path, p: &Pin, progress: &Progress) -> Result<()> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(zip).map_err(err)?).map_err(err)?;
    if archive.len() > 100_000 {
        return Err("Runtime archive has too many entries.".into());
    }
    let mut paths = vec![];
    let mut top = None;
    let mut wrapped = true;
    let mut bytes = 0u64;
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(err)?;
        let path = crate::import::safe_name(&entry.name().map_err(err)?).map_err(err)?;
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err("Runtime archive contains a symbolic link.".into());
        }
        let first = path.components().next().unwrap().as_os_str().to_owned();
        if top.as_ref().is_some_and(|t| t != &first) {
            wrapped = false;
        } else if top.is_none() {
            top = Some(first);
        }
        if path.components().count() == 1 && !entry.is_dir() {
            wrapped = false;
        }
        bytes = bytes
            .checked_add(entry.size())
            .ok_or("Runtime archive is too large")?;
        if bytes > p.installed_estimate * 3 || entry.size() > p.installed_estimate * 2 {
            return Err("Runtime archive exceeds its expanded size limit.".into());
        }
        paths.push(path);
    }
    std::fs::create_dir_all(stage).map_err(err)?;
    let mut seen = HashSet::new();
    let mut done = 0;
    for (i, path) in paths.into_iter().enumerate() {
        let relative = if wrapped {
            path.components().skip(1).collect::<PathBuf>()
        } else {
            path
        };
        if relative.as_os_str().is_empty() {
            continue;
        }
        if !seen.insert(relative.to_string_lossy().to_ascii_lowercase()) {
            return Err("Runtime archive contains duplicate paths.".into());
        }
        let mut entry = archive.by_index(i).map_err(err)?;
        let destination = stage.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(destination).map_err(err)?;
            continue;
        }
        std::fs::create_dir_all(destination.parent().unwrap()).map_err(err)?;
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)
            .map_err(err)?;
        let expected = entry.size();
        let copied =
            std::io::copy(&mut entry.by_ref().take(expected + 1), &mut out).map_err(err)?;
        if copied != expected {
            return Err("Runtime archive entry is truncated or exceeds its size.".into());
        }
        done += copied;
        progress("extract", done, Some(bytes));
    }
    Ok(())
}
#[cfg(windows)]
fn free_space(path: &Path) -> Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetDiskFreeSpaceExW(
            directory: *const u16,
            available: *mut u64,
            total: *mut u64,
            total_free: *mut u64,
        ) -> i32;
    }
    let path = std::fs::canonicalize(path).map_err(err)?;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut free = 0;
    if unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(err(std::io::Error::last_os_error()));
    }
    Ok(free)
}
#[cfg(not(windows))]
fn free_space(_: &Path) -> Result<u64> {
    Err("Runtime installation requires Windows.".into())
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };
    struct Probe;
    impl ProbeRunner for Probe {
        fn version(&self, path: &Path, arg: &str) -> Result<String> {
            assert!(path.is_file());
            assert_eq!(arg, "--version");
            Ok("v24.22.2".into())
        }
    }
    fn fixture(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, bytes) in entries {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }
    fn manager(root: &Path, urls: Vec<String>, bytes: &[u8], free: u64) -> Runtimes {
        let pin = Pin {
            kind: "node",
            version: "24.22.2",
            urls,
            sha256: hex::encode(Sha256::digest(bytes)),
            executable: "node.exe",
            download_limit: MIB,
            installed_estimate: MIB,
        };
        Runtimes::with_parts(
            root,
            vec![pin],
            reqwest::Client::new(),
            Arc::new(Probe),
            Arc::new(move |_| Ok(free)),
        )
    }
    fn progress() -> Arc<Progress> {
        Arc::new(|_, _, _| {})
    }
    #[tokio::test]
    async fn good_streamed_install_list_resolve_remove() {
        let server = MockServer::start().await;
        let tmp = tempfile::tempdir().unwrap();
        let bytes = fixture(&[("node-v24/node.exe", b"fixture")]);
        Mock::given(method("GET"))
            .and(path("/runtime"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes.clone()))
            .expect(1)
            .mount(&server)
            .await;
        let manager = manager(
            tmp.path(),
            vec![format!("{}/runtime", server.uri())],
            &bytes,
            u64::MAX,
        );
        let phases = Arc::new(Mutex::new(Vec::new()));
        let captured = phases.clone();
        manager
            .install(
                "node",
                "24.22.2",
                Arc::new(move |phase, _, _| captured.lock().unwrap().push(phase.to_owned())),
            )
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(manager.resolve("node", "24.22.2").unwrap()).unwrap(),
            b"fixture"
        );
        for phase in ["download", "verify", "extract", "check"] {
            assert!(phases.lock().unwrap().iter().any(|p| p == phase));
        }
        let statuses = manager
            .list(HashMap::from([(
                ("node".into(), "24.22.2".into()),
                vec!["site".into()],
            )]))
            .await
            .unwrap();
        assert!(statuses[0].ready);
        assert_eq!(statuses[0].size_bytes, Some(7));
        assert_eq!(statuses[0].used_by, ["site"]);
        assert!(
            manager
                .remove("node", "24.22.2", &["site".into()])
                .await
                .unwrap_err()
                .contains("site")
        );
        manager.remove("node", "24.22.2", &[]).await.unwrap();
        assert!(manager.resolve("node", "24.22.2").is_err());
        assert_eq!(
            std::fs::read_dir(tmp.path().join("runtimes/.partial"))
                .unwrap()
                .count(),
            0
        );
    }
    #[tokio::test]
    async fn hash_failure_preserves_old_and_never_falls_back() {
        let server = MockServer::start().await;
        let tmp = tempfile::tempdir().unwrap();
        let bytes = fixture(&[("node.exe", b"fixture")]);
        Mock::given(path("/bad"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"wrong"))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(path("/archive"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes.clone()))
            .expect(0)
            .mount(&server)
            .await;
        let manager = manager(
            tmp.path(),
            vec![
                format!("{}/bad", server.uri()),
                format!("{}/archive", server.uri()),
            ],
            &bytes,
            u64::MAX,
        );
        let dir = manager.directory(&manager.pins[0]);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("node.exe"), b"previous").unwrap();
        assert!(
            manager
                .install("node", "24.22.2", progress())
                .await
                .unwrap_err()
                .contains("integrity check")
        );
        assert_eq!(std::fs::read(dir.join("node.exe")).unwrap(), b"previous");
        assert_eq!(
            std::fs::read_dir(tmp.path().join("runtimes/.partial"))
                .unwrap()
                .count(),
            0
        );
    }
    #[tokio::test]
    async fn missing_primary_falls_back() {
        let server = MockServer::start().await;
        let tmp = tempfile::tempdir().unwrap();
        let bytes = fixture(&[("node.exe", b"fixture")]);
        Mock::given(path("/primary"))
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(path("/archive"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes.clone()))
            .expect(1)
            .mount(&server)
            .await;
        manager(
            tmp.path(),
            vec![
                format!("{}/primary", server.uri()),
                format!("{}/archive", server.uri()),
            ],
            &bytes,
            u64::MAX,
        )
        .install("node", "24.22.2", progress())
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn traversal_and_low_space_and_unknown_rejected() {
        let server = MockServer::start().await;
        let tmp = tempfile::tempdir().unwrap();
        let bytes = fixture(&[("../escape", b"evil"), ("node.exe", b"fixture")]);
        Mock::given(path("/runtime"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes.clone()))
            .expect(1)
            .mount(&server)
            .await;
        let mut manager = manager(
            tmp.path(),
            vec![format!("{}/runtime", server.uri())],
            &bytes,
            0,
        );
        let error = manager
            .install("node", "24.22.2", progress())
            .await
            .unwrap_err();
        assert!(error.contains("needs"));
        assert!(error.contains("0 bytes are free"));
        assert!(
            manager
                .install("node", "../../escape", progress())
                .await
                .unwrap_err()
                .contains("Unknown")
        );
        manager.free = Arc::new(|_| Ok(u64::MAX));
        assert!(
            manager
                .install("node", "24.22.2", progress())
                .await
                .is_err()
        );
        assert!(!tmp.path().join("escape").exists());
        assert!(manager.resolve("node", "24.22.2").is_err());
        assert_eq!(
            std::fs::read_dir(tmp.path().join("runtimes/.partial"))
                .unwrap()
                .count(),
            0
        );
    }
    #[tokio::test]
    async fn truncated_transport_fails() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request).await.unwrap();
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\nConnection: close\r\n\r\ntruncated").await.unwrap();
        });
        let tmp = tempfile::tempdir().unwrap();
        let manager = manager(
            tmp.path(),
            vec![format!("http://{address}/runtime")],
            b"expected",
            u64::MAX,
        );
        assert!(
            manager
                .install("node", "24.22.2", progress())
                .await
                .unwrap_err()
                .contains("truncated")
        );
        task.await.unwrap();
        assert!(manager.resolve("node", "24.22.2").is_err());
        assert_eq!(
            std::fs::read_dir(tmp.path().join("runtimes/.partial"))
                .unwrap()
                .count(),
            0
        );
    }
    #[tokio::test]
    async fn serialization_partial_cleanup_and_mysql_data_guard() {
        let tmp = tempfile::tempdir().unwrap();
        let mut manager = manager(tmp.path(), vec![], b"", u64::MAX);
        let partial = tmp.path().join("runtimes/.partial/node-24.22.2");
        std::fs::create_dir_all(&partial).unwrap();
        std::fs::write(partial.join("interrupted"), b"x").unwrap();
        manager.clean_partial().unwrap();
        assert!(!partial.exists());
        let lock = manager.locks[0].lock().await;
        assert!(
            manager
                .install("node", "24.22.2", progress())
                .await
                .unwrap_err()
                .contains("already")
        );
        drop(lock);
        manager.pins[0].kind = "mysql";
        std::fs::create_dir_all(tmp.path().join("mysql/data")).unwrap();
        assert!(
            manager
                .remove("mysql", "24.22.2", &[])
                .await
                .unwrap_err()
                .contains("data directory")
        );
    }
    #[test]
    fn manifest_hosts_and_visual_cpp_errors() {
        assert_eq!(MANIFEST.len(), 5);
        for pin in MANIFEST {
            for url in pin.urls {
                let parsed = reqwest::Url::parse(url).unwrap();
                assert_eq!(parsed.scheme(), "https");
                assert!(
                    ["nodejs.org", "windows.php.net", "cdn.mysql.com"]
                        .contains(&parsed.host_str().unwrap())
                );
            }
        }
        for message in [
            "0xC0000135",
            "-1073741515",
            "VCRUNTIME140.dll",
            "3221225781",
        ] {
            assert!(probe_error(message.into()).contains("Microsoft Visual C++ runtime is absent"));
        }
    }
}
#[cfg(test)]
mod marker_tests {
    use super::*;
    #[test]
    fn copied_tree_keeps_nested_files() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("a");
        std::fs::create_dir_all(from.join("lib/enchant")).unwrap();
        std::fs::write(from.join("lib/enchant/x.dll"), b"x").unwrap();
        copy_tree(&from, &dir.path().join("b")).unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("b/lib/enchant/x.dll")).unwrap(),
            b"x"
        );
    }
    #[test]
    fn a_runtime_without_the_marker_is_not_installed() {
        let dir = tempfile::tempdir().unwrap();
        let runtimes = Runtimes::new(dir.path()).unwrap();
        let install = dir.path().join("runtimes/node/24.22.2");
        std::fs::create_dir_all(&install).unwrap();
        std::fs::write(install.join("node.exe"), b"interrupted copy").unwrap();
        assert!(runtimes.resolve("node", "24.22.2").is_err());
        std::fs::write(install.join(INSTALLED_MARKER), b"x").unwrap();
        assert!(runtimes.resolve("node", "24.22.2").is_ok());
    }
}
#[cfg(test)]
mod live_install {
    use super::*;
    /// Real downloads from the publishers. Set HOSTIFY_LIVE_RUNTIMES to a directory to keep them.
    #[tokio::test]
    #[ignore = "downloads about 500 MB from the real publishers"]
    async fn every_pinned_runtime_installs_and_reports_its_version() {
        let root = PathBuf::from(std::env::var("HOSTIFY_LIVE_RUNTIMES").unwrap());
        let runtimes = Runtimes::new(&root).unwrap();
        for (kind, version) in MANIFEST.iter().map(|p| (p.kind, p.version)) {
            if runtimes.resolve(kind, version).is_err() {
                let started = Instant::now();
                runtimes
                    .install(kind, version, Arc::new(|_, _, _| {}))
                    .await
                    .unwrap_or_else(|e| panic!("{kind} {version}: {e}"));
                println!("installed {kind} {version} in {:?}", started.elapsed());
            }
        }
        for status in runtimes.list(HashMap::new()).await.unwrap() {
            println!(
                "{} {} ready={} size={:?} error={:?}",
                status.kind, status.version, status.ready, status.size_bytes, status.error
            );
            assert!(status.ready, "{} {}", status.kind, status.version);
        }
    }
}
