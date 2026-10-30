use async_trait::async_trait;
use sha2::{Digest, Sha256, Sha512};
use std::{
    borrow::Cow,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    sync::Arc,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ComponentError {
    #[error("Download failed: {0}")]
    Download(String),
    #[error("Missing or invalid checksum for {0}")]
    Checksum(String),
    #[error("Component archive error: {0}")]
    Archive(#[from] zip::result::ZipError),
    #[error("Component I/O error: {0}")]
    Io(#[from] std::io::Error),
}
#[async_trait]
pub trait Http: Send + Sync {
    async fn get(&self, url: &str) -> Result<Vec<u8>, ComponentError>;
}
pub struct ReqwestHttp(pub reqwest::Client);
const MAX_DOWNLOAD: usize = 256 * 1024 * 1024;
fn download_error(e: impl std::fmt::Display) -> ComponentError {
    ComponentError::Download(format!(
        "Cannot download components. Check your internet connection and retry: {e}"
    ))
}
#[async_trait]
impl Http for ReqwestHttp {
    async fn get(&self, url: &str) -> Result<Vec<u8>, ComponentError> {
        let mut r = self
            .0
            .get(url)
            .header("User-Agent", "Hostify")
            .send()
            .await
            .map_err(download_error)?;
        if matches!(r.status().as_u16(), 403 | 429) {
            return Err(ComponentError::Download("GitHub rate limit or access restriction. Wait a few minutes and retry installing components.".into()));
        }
        r = r.error_for_status().map_err(download_error)?;
        if r.content_length().is_some_and(|n| n > MAX_DOWNLOAD as u64) {
            return Err(download_error("Download exceeds 256 MiB"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = r.chunk().await.map_err(download_error)? {
            if bytes.len().saturating_add(chunk.len()) > MAX_DOWNLOAD {
                return Err(download_error("Download exceeds 256 MiB"));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
}
/// Reviewed publisher pins. To bump: download, verify against the publisher's
/// checksum list, then update this table (including every listed digest).
#[derive(Clone, Debug)]
struct ComponentPin {
    name: &'static str,
    version: &'static str,
    asset: &'static str,
    url: &'static str,
    sha256: Cow<'static, str>,
    sha512: Option<Cow<'static, str>>,
}
const PINNED_COMPONENTS: &[ComponentPin] = &[
    ComponentPin {
        name: "caddy",
        version: "v2.11.7",
        asset: "caddy_2.11.7_windows_amd64.zip",
        url: "https://github.com/caddyserver/caddy/releases/download/v2.11.7/caddy_2.11.7_windows_amd64.zip",
        sha256: Cow::Borrowed("0a1edc0b799512051c57071ce0e798d3f2cf67dc98171366f1d2b326072e3b06"),
        sha512: Some(Cow::Borrowed(
            "c308154504e53755958ffc62d8a2657ab17eefd6dd25c45dbb2c3d93d29519971f77ca652f7d0f2bcb7f980b9ba3822478865e29969d0272d7613550a43152b1",
        )),
    },
    ComponentPin {
        name: "cloudflared",
        version: "2026.10.6",
        asset: "cloudflared-windows-amd64.exe",
        url: "https://github.com/cloudflare/cloudflared/releases/download/2026.10.6/cloudflared-windows-amd64.exe",
        sha256: Cow::Borrowed("d521b3bf68982165f0d06d76732f22d9a105ef057ff3ae4c0986f3a23190950f"),
        sha512: None,
    },
];
pub struct Components {
    pub http: Arc<dyn Http>,
    pub bin: PathBuf,
    #[cfg(test)]
    manifest_override: Option<Vec<ComponentPin>>,
}
impl Components {
    pub fn new(bin: PathBuf) -> Result<Self, ComponentError> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(download_error)?;
        Ok(Self {
            http: Arc::new(ReqwestHttp(client)),
            bin,
            #[cfg(test)]
            manifest_override: None,
        })
    }
    fn pin(&self, name: &str) -> Result<ComponentPin, ComponentError> {
        executable(name)?;
        #[cfg(test)]
        if let Some(pins) = &self.manifest_override {
            return pins
                .iter()
                .find(|p| p.name == name)
                .cloned()
                .ok_or_else(|| download_error("Missing component pin"));
        }
        PINNED_COMPONENTS
            .iter()
            .find(|p| p.name == name)
            .cloned()
            .ok_or_else(|| download_error("Missing component pin"))
    }
    async fn download(&self, url: &str) -> Result<Vec<u8>, ComponentError> {
        let bytes = self.http.get(url).await?;
        if bytes.len() > MAX_DOWNLOAD {
            return Err(download_error("Download exceeds 256 MiB"));
        }
        Ok(bytes)
    }
    pub async fn ensure(
        &self,
        names: &[&str],
        runner: Arc<dyn ProbeRunner>,
        progress: impl Fn(&str, u8),
    ) -> Result<(), ComponentError> {
        // Validate the complete request before performing any downloads.
        for name in names {
            executable(name)?;
        }
        for name in names {
            let bin = self.bin.clone();
            let name_owned = name.to_string();
            let probe_runner = runner.clone();
            let status = tokio::task::spawn_blocking(move || {
                probe(&bin, &name_owned, probe_runner.as_ref())
            })
            .await
            .map_err(download_error)?;
            if !status.ready {
                self.install_with_runner(name, runner.clone(), &progress)
                    .await?;
            } else {
                progress(name, 100);
            }
        }
        Ok(())
    }
    pub async fn version(&self, name: &str) -> Result<String, ComponentError> {
        let path = self.bin.join(executable(name)?);
        let arg = if name == "caddy" {
            "version"
        } else {
            "--version"
        };
        tokio::task::spawn_blocking(move || ProcessProbe.version(&path, arg))
            .await
            .map_err(download_error)?
            .map_err(download_error)
    }
    pub async fn install(
        &self,
        name: &str,
        progress: impl Fn(&str, u8),
    ) -> Result<String, ComponentError> {
        self.install_with_runner(name, Arc::new(ProcessProbe), progress)
            .await
    }
    async fn install_with_runner(
        &self,
        name: &str,
        runner: Arc<dyn ProbeRunner>,
        progress: impl Fn(&str, u8),
    ) -> Result<String, ComponentError> {
        let pin = self.pin(name)?;
        progress(name, 0);
        progress(name, 20);
        let bytes = self.download(pin.url).await?;
        let valid = |hash: &str, len| {
            hash.len() == len
                && hash
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        if !valid(&pin.sha256, 64) || hex::encode(Sha256::digest(&bytes)) != pin.sha256 {
            return Err(ComponentError::Checksum(format!("{} (SHA-256)", pin.asset)));
        }
        if let Some(expected) = &pin.sha512 {
            if !valid(expected, 128) || hex::encode(Sha512::digest(&bytes)) != *expected {
                return Err(ComponentError::Checksum(format!("{} (SHA-512)", pin.asset)));
            }
        }
        progress(name, 80);
        let executable_bytes = if name == "caddy" {
            let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
            let file = archive.by_name(crate::paths::CADDY_EXE)?;
            if file.size() > MAX_DOWNLOAD as u64 {
                return Err(download_error("Expanded executable exceeds 256 MiB"));
            }
            let mut buf = Vec::new();
            file.take(MAX_DOWNLOAD as u64 + 1).read_to_end(&mut buf)?;
            if buf.len() > MAX_DOWNLOAD {
                return Err(download_error("Expanded executable exceeds 256 MiB"));
            }
            buf
        } else {
            bytes
        };
        std::fs::create_dir_all(&self.bin)?;
        let tmp = self
            .bin
            .join(format!(".{name}-{}.tmp", uuid::Uuid::new_v4()));
        if let Err(e) = std::fs::write(&tmp, executable_bytes) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        let target = self.bin.join(executable(name)?);
        if let Err(e) = replace(&tmp, &target) {
            let _ = std::fs::remove_file(&tmp);
            return Err(ComponentError::Download(format!(
                "Cannot replace {}: {e}. Stop websites or tunnels using this component, then retry Install components.",
                target.display()
            )));
        }
        let bin = self.bin.clone();
        let name_owned = name.to_owned();
        let status = tokio::task::spawn_blocking(move || probe(&bin, &name_owned, runner.as_ref()))
            .await
            .map_err(download_error)?;
        let version = status.version.filter(|_| status.ready).ok_or_else(|| {
            ComponentError::Download(format!(
                "Installed {name}, but its version probe failed: {}",
                status.error.unwrap_or_default()
            ))
        })?;
        if !version.contains(pin.version) {
            return Err(ComponentError::Download(format!(
                "Warning: installed {name} reports {version:?}, which does not contain pinned version {}",
                pin.version
            )));
        }
        progress(name, 100);
        Ok(version)
    }
}
fn replace(tmp: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::rename(tmp, target)
}
pub const CADDY_UNAVAILABLE: &str = "Caddy (the web server) is not installed or can't run. Open the Dashboard and choose \"Install components\", then start the website again.";
pub fn executable(name: &str) -> Result<&'static str, ComponentError> {
    match name {
        "caddy" => Ok(crate::paths::CADDY_EXE),
        "cloudflared" => Ok(crate::paths::CLOUDFLARED_EXE),
        _ => Err(download_error("Unknown component")),
    }
}
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentStatus {
    pub installed: bool,
    pub ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
pub trait ProbeRunner: Send + Sync {
    fn version(&self, path: &Path, arg: &str) -> Result<String, String>;
}
pub struct ProcessProbe;
impl ProbeRunner for ProcessProbe {
    fn version(&self, path: &Path, arg: &str) -> Result<String, String> {
        use std::{
            process::{Command, Stdio},
            time::{Duration, Instant},
        };
        let mut cmd = Command::new(path);
        cmd.arg(arg)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000);
        }
        let mut child = cmd.spawn().map_err(|e| e.to_string())?;
        let stdout = child.stdout.take().ok_or("Version output unavailable")?;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = stdout
                .take(16 * 1024)
                .read_to_end(&mut bytes)
                .map(|_| String::from_utf8_lossy(&bytes).trim().to_owned());
            let _ = tx.send(result);
        });
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    return if status.success() {
                        rx.recv_timeout(Duration::from_millis(100))
                            .map_err(|e| e.to_string())?
                            .map_err(|e| e.to_string())
                    } else {
                        Err(format!("Version command exited with {status}"))
                    };
                }
                Ok(None) if started.elapsed() < Duration::from_secs(5) => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                result => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(match result {
                        Err(e) => e.to_string(),
                        _ => "Version probe timed out after 5 seconds".into(),
                    });
                }
            }
        }
    }
}
pub fn probe(bin: &Path, name: &str, runner: &dyn ProbeRunner) -> ComponentStatus {
    let path = bin.join(executable(name).expect("known component"));
    let installed = path.is_file();
    let result = if !installed {
        Err("Not installed".into())
    } else if std::fs::metadata(&path)
        .map(|m| m.len() == 0)
        .unwrap_or(true)
    {
        Err("Executable is empty or unreadable. Reinstall this component.".into())
    } else {
        runner.version(
            &path,
            if name == "caddy" {
                "version"
            } else {
                "--version"
            },
        )
    };
    match result {
        Ok(version) => ComponentStatus {
            installed,
            ready: true,
            version: Some(version),
            path: path.to_string_lossy().into_owned(),
            error: None,
        },
        Err(error) => ComponentStatus {
            installed,
            ready: false,
            version: None,
            path: path.to_string_lossy().into_owned(),
            error: Some(error),
        },
    }
}
/// Serializes concurrent refreshes, caches briefly, and never probes on the async executor.
pub struct Detection {
    bin: PathBuf,
    runner: Arc<dyn ProbeRunner>,
    cache: tokio::sync::Mutex<Option<(std::time::Instant, [FileStamp; 2], [ComponentStatus; 2])>>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct FileStamp {
    exists: bool,
    length: u64,
    modified: Option<std::time::SystemTime>,
}
fn file_stamps(bin: &Path) -> [FileStamp; 2] {
    ["caddy", "cloudflared"].map(|name| {
        match std::fs::metadata(bin.join(executable(name).expect("known component"))) {
            Ok(m) => FileStamp {
                exists: m.is_file(),
                length: m.len(),
                modified: m.modified().ok(),
            },
            Err(_) => FileStamp {
                exists: false,
                length: 0,
                modified: None,
            },
        }
    })
}
impl Detection {
    pub fn new(bin: PathBuf, runner: Arc<dyn ProbeRunner>) -> Self {
        Self {
            bin,
            runner,
            cache: tokio::sync::Mutex::new(None),
        }
    }
    pub fn runner(&self) -> Arc<dyn ProbeRunner> {
        self.runner.clone()
    }
    pub async fn invalidate(&self) {
        *self.cache.lock().await = None;
    }
    pub async fn statuses(&self, fresh: bool) -> [ComponentStatus; 2] {
        let mut cache = self.cache.lock().await;
        let stamps = file_stamps(&self.bin);
        if !fresh {
            if let Some((at, key, status)) = &*cache {
                if *key == stamps && at.elapsed() < std::time::Duration::from_secs(2) {
                    return status.clone();
                }
            }
        }
        let bin = self.bin.clone();
        let runner = self.runner.clone();
        let statuses = tokio::task::spawn_blocking(move || {
            ["caddy", "cloudflared"].map(|name| probe(&bin, name, runner.as_ref()))
        })
        .await
        .expect("component probe task");
        *cache = Some((std::time::Instant::now(), stamps, statuses.clone()));
        statuses
    }
    pub async fn caddy_ready(&self) -> bool {
        let bin = self.bin.clone();
        let runner = self.runner.clone();
        tokio::task::spawn_blocking(move || probe(&bin, "caddy", runner.as_ref()).ready)
            .await
            .unwrap_or(false)
    }
}
#[cfg(test)]
mod regression_tests {
    use super::*;
    use std::{
        collections::HashMap,
        io::Write,
        sync::atomic::{AtomicUsize, Ordering},
    };
    struct Runner {
        calls: AtomicUsize,
        fails: bool,
    }
    impl ProbeRunner for Runner {
        fn version(&self, _: &Path, _: &str) -> Result<String, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fails {
                Err("cannot execute".into())
            } else {
                Ok("v-test".into())
            }
        }
    }
    fn runner(fails: bool) -> Arc<Runner> {
        Arc::new(Runner {
            calls: AtomicUsize::new(0),
            fails,
        })
    }
    #[test]
    fn component_status_matrix() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path();
        let r = runner(false);
        for name in ["caddy", "cloudflared"] {
            let s = probe(bin, name, r.as_ref());
            assert!(!s.installed && !s.ready);
        }
        std::fs::write(bin.join(crate::paths::CADDY_EXE), b"caddy").unwrap();
        assert!(probe(bin, "caddy", r.as_ref()).ready);
        assert!(!probe(bin, "cloudflared", r.as_ref()).ready);
        std::fs::remove_file(bin.join(crate::paths::CADDY_EXE)).unwrap();
        std::fs::write(bin.join(crate::paths::CLOUDFLARED_EXE), b"cloudflared").unwrap();
        assert!(!probe(bin, "caddy", r.as_ref()).ready);
        assert!(probe(bin, "cloudflared", r.as_ref()).ready);
        std::fs::write(bin.join(crate::paths::CADDY_EXE), b"").unwrap();
        let s = probe(bin, "caddy", r.as_ref());
        assert!(s.installed && !s.ready);
        assert!(s.error.unwrap().contains("empty"));
        std::fs::write(bin.join(crate::paths::CADDY_EXE), b"corrupt").unwrap();
        let s = probe(bin, "caddy", runner(true).as_ref());
        assert!(s.installed && !s.ready);
        assert_eq!(s.error.as_deref(), Some("cannot execute"));
    }
    #[tokio::test]
    async fn cache_refresh_and_invalidation() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(crate::paths::CADDY_EXE), b"caddy").unwrap();
        let r = runner(false);
        let d = Detection::new(dir.path().into(), r.clone());
        assert!(d.statuses(false).await[0].ready);
        d.statuses(false).await;
        assert_eq!(r.calls.load(Ordering::SeqCst), 1);
        d.invalidate().await;
        d.statuses(false).await;
        assert_eq!(r.calls.load(Ordering::SeqCst), 2);
        // Age the cache deterministically to retain expiry coverage without sleeping.
        d.cache.lock().await.as_mut().unwrap().0 =
            std::time::Instant::now() - std::time::Duration::from_secs(3);
        d.statuses(false).await;
        assert_eq!(r.calls.load(Ordering::SeqCst), 3);
        let path = dir.path().join(crate::paths::CADDY_EXE);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(123))
            .unwrap();
        d.statuses(false).await;
        assert_eq!(r.calls.load(Ordering::SeqCst), 4);
        let path = dir.path().join(crate::paths::CADDY_EXE);
        std::fs::write(&path, b"").unwrap();
        assert!(!d.statuses(false).await[0].ready);
        std::fs::write(&path, b"replacement").unwrap();
        assert!(d.statuses(false).await[0].ready);
        std::fs::remove_file(&path).unwrap();
        assert!(!d.statuses(false).await[0].installed);
    }
    struct MockHttp(HashMap<String, Vec<u8>>);
    #[async_trait]
    impl Http for MockHttp {
        async fn get(&self, url: &str) -> Result<Vec<u8>, ComponentError> {
            self.0
                .get(url)
                .cloned()
                .ok_or_else(|| download_error("offline mock"))
        }
    }
    fn caddy(bin: &Path, entries: &[(&str, &[u8])], bad_checksum: bool) -> Components {
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in entries {
            archive
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(bytes).unwrap();
        }
        let bytes = archive.finish().unwrap().into_inner();
        let sha256 = hex::encode(Sha256::digest(&bytes));
        let sha512 = if bad_checksum {
            "0".repeat(128)
        } else {
            hex::encode(Sha512::digest(&bytes))
        };
        let pin = ComponentPin {
            name: "caddy",
            version: "v-test",
            asset: "test.zip",
            url: PINNED_COMPONENTS[0].url,
            sha256: sha256.into(),
            sha512: Some(sha512.into()),
        };
        Components {
            http: Arc::new(MockHttp(HashMap::from([(pin.url.into(), bytes)]))),
            bin: bin.into(),
            manifest_override: Some(vec![pin]),
        }
    }
    struct RecordingHttp {
        url: &'static str,
        bytes: Vec<u8>,
        calls: std::sync::Mutex<Vec<String>>,
    }
    #[async_trait]
    impl Http for RecordingHttp {
        async fn get(&self, url: &str) -> Result<Vec<u8>, ComponentError> {
            self.calls.lock().unwrap().push(url.to_owned());
            assert_eq!(url, self.url, "only the pinned asset URL may be requested");
            assert!(!url.contains("/repos/"));
            Ok(self.bytes.clone())
        }
    }
    #[tokio::test]
    async fn installs_both_pins_with_only_one_asset_request_and_reports_probed_version() {
        for name in ["caddy", "cloudflared"] {
            let dir = tempfile::tempdir().unwrap();
            let mut c = caddy(dir.path(), &[("caddy.exe", b"fake executable")], false);
            let mut pin = c.manifest_override.as_ref().unwrap()[0].clone();
            let bytes = if name == "caddy" {
                c.http.get(pin.url).await.unwrap()
            } else {
                pin = PINNED_COMPONENTS[1].clone();
                pin.version = "v-test";
                pin.sha256 = hex::encode(Sha256::digest(b"fake executable")).into();
                b"fake executable".to_vec()
            };
            let http = Arc::new(RecordingHttp {
                url: pin.url,
                bytes,
                calls: std::sync::Mutex::new(Vec::new()),
            });
            c.http = http.clone();
            c.manifest_override = Some(vec![pin.clone()]);
            let r = runner(false);
            assert_eq!(
                c.install_with_runner(name, r.clone(), |_, _| {})
                    .await
                    .unwrap(),
                "v-test"
            );
            assert_eq!(r.calls.load(Ordering::SeqCst), 1);
            assert_eq!(
                std::fs::read(dir.path().join(executable(name).unwrap())).unwrap(),
                b"fake executable"
            );
            assert_eq!(*http.calls.lock().unwrap(), vec![pin.url.to_owned()]);
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        }
    }
    #[tokio::test]
    async fn refuses_bad_checksum_without_installing() {
        let dir = tempfile::tempdir().unwrap();
        let mut pin = PINNED_COMPONENTS[1].clone();
        pin.sha256 = "0".repeat(64).into();
        let http = Arc::new(RecordingHttp {
            url: pin.url,
            bytes: b"wrong".to_vec(),
            calls: std::sync::Mutex::new(Vec::new()),
        });
        let c = Components {
            http: http.clone(),
            bin: dir.path().into(),
            manifest_override: Some(vec![pin.clone()]),
        };
        assert!(matches!(
            c.install("cloudflared", |_, _| {}).await,
            Err(ComponentError::Checksum(_))
        ));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        assert_eq!(*http.calls.lock().unwrap(), vec![pin.url.to_owned()]);
    }
    #[tokio::test]
    async fn offline_download_preserves_existing_binary_without_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(crate::paths::CADDY_EXE);
        std::fs::write(&target, b"working").unwrap();
        let c = Components {
            http: Arc::new(MockHttp(HashMap::new())),
            bin: dir.path().into(),
            manifest_override: None,
        };
        let error = c.install("caddy", |_, _| {}).await.unwrap_err().to_string();
        assert!(error.contains("internet connection") && error.contains("retry"));
        assert_eq!(std::fs::read(target).unwrap(), b"working");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    #[tokio::test]
    async fn wrong_sha256_preserves_existing_binary_even_with_correct_sha512() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(crate::paths::CADDY_EXE);
        std::fs::write(&target, b"working").unwrap();
        let mut c = caddy(dir.path(), &[("caddy.exe", b"replacement")], false);
        c.manifest_override.as_mut().unwrap()[0].sha256 = "0".repeat(64).into();
        let error = c.install("caddy", |_, _| {}).await.unwrap_err();
        assert!(matches!(error, ComponentError::Checksum(_)));
        assert!(error.to_string().contains("SHA-256"));
        assert_eq!(std::fs::read(target).unwrap(), b"working");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    #[tokio::test]
    async fn post_install_probe_failure_and_unexpected_version_are_errors() {
        for fails in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let mut c = caddy(dir.path(), &[("caddy.exe", b"replacement")], false);
            c.manifest_override.as_mut().unwrap()[0].version = "unexpected-version";
            let error = c
                .install_with_runner("caddy", runner(fails), |_, _| {})
                .await
                .unwrap_err()
                .to_string();
            if fails {
                assert!(error.contains("version probe failed"));
            } else {
                assert!(
                    error.contains("Warning:")
                        && error.contains("unexpected-version")
                        && error.contains("v-test")
                );
            }
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        }
    }
    #[tokio::test]
    async fn ensure_repairs_missing_and_non_running_binaries() {
        struct ContentProbe;
        impl ProbeRunner for ContentProbe {
            fn version(&self, path: &Path, _: &str) -> Result<String, String> {
                if std::fs::read(path).unwrap() == b"replacement" {
                    Ok("v-test".into())
                } else {
                    Err("cannot execute".into())
                }
            }
        }
        for present in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let target = dir.path().join(crate::paths::CADDY_EXE);
            if present {
                std::fs::write(&target, b"corrupt").unwrap();
            }
            let c = caddy(dir.path(), &[("caddy.exe", b"replacement")], false);
            c.ensure(&["caddy"], Arc::new(ContentProbe), |_, _| {})
                .await
                .unwrap();
            assert_eq!(std::fs::read(target).unwrap(), b"replacement");
        }
    }
    #[tokio::test]
    async fn ensure_components_reinstalls_zero_byte_caddy() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(crate::paths::CADDY_EXE);
        std::fs::write(&target, b"").unwrap();
        let c = caddy(dir.path(), &[("caddy.exe", b"replacement")], false);
        let progress = std::sync::Mutex::new(Vec::new());
        c.ensure(&["caddy"], runner(false), |n, p| {
            progress.lock().unwrap().push((n.to_owned(), p))
        })
        .await
        .unwrap();
        assert_eq!(std::fs::read(target).unwrap(), b"replacement");
        assert!(!dir.path().join(crate::paths::CLOUDFLARED_EXE).exists());
        assert_eq!(
            *progress.lock().unwrap(),
            vec![
                ("caddy".into(), 0),
                ("caddy".into(), 20),
                ("caddy".into(), 80),
                ("caddy".into(), 100)
            ]
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    #[tokio::test]
    async fn failed_checksum_preserves_existing_binary_without_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(crate::paths::CADDY_EXE);
        std::fs::write(&target, b"working").unwrap();
        let error = caddy(dir.path(), &[("caddy.exe", b"wrong")], true)
            .install_with_runner("caddy", runner(false), |_, _| {})
            .await
            .unwrap_err();
        assert!(matches!(error, ComponentError::Checksum(_)));
        assert!(error.to_string().contains("SHA-512"));
        assert_eq!(std::fs::read(target).unwrap(), b"working");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    #[tokio::test]
    async fn zip_traversal_entries_only_extract_expected_caddy() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        caddy(
            &bin,
            &[
                ("../escape.exe", b"bad"),
                ("nested/caddy.exe", b"bad"),
                ("caddy.exe", b"good"),
            ],
            false,
        )
        .install_with_runner("caddy", runner(false), |_, _| {})
        .await
        .unwrap();
        assert!(!dir.path().join("escape.exe").exists());
        assert_eq!(std::fs::read_dir(&bin).unwrap().count(), 1);
        assert_eq!(
            std::fs::read(bin.join(crate::paths::CADDY_EXE)).unwrap(),
            b"good"
        );
    }
    #[tokio::test]
    async fn traversal_only_zip_cannot_replace_existing_binary() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(crate::paths::CADDY_EXE);
        std::fs::write(&target, b"working").unwrap();
        assert!(
            caddy(dir.path(), &[("../caddy.exe", b"bad")], false)
                .install_with_runner("caddy", runner(false), |_, _| {})
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(target).unwrap(), b"working");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    #[tokio::test]
    async fn ensure_preserves_ready_binary_without_downloads() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(crate::paths::CADDY_EXE), b"working").unwrap();
        let c = Components {
            http: Arc::new(MockHttp(HashMap::new())),
            bin: dir.path().into(),
            manifest_override: None,
        };
        c.ensure(&["caddy"], runner(false), |_, _| {})
            .await
            .unwrap();
    }
    #[cfg(windows)]
    #[tokio::test]
    async fn locked_target_returns_action_and_cleans_temporary_file() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(crate::paths::CADDY_EXE);
        std::fs::write(&target, b"old").unwrap();
        let _lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&target)
            .unwrap();
        let error = caddy(dir.path(), &[("caddy.exe", b"replacement")], false)
            .install_with_runner("caddy", runner(false), |_, _| {})
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Stop websites or tunnels"));
        assert_eq!(std::fs::read(target).unwrap(), b"old");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    #[test]
    fn real_probe_rejects_non_executable_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(crate::paths::CADDY_EXE), b"corrupt").unwrap();
        let status = probe(dir.path(), "caddy", &ProcessProbe);
        assert!(status.installed && !status.ready);
        assert!(status.error.is_some());
    }
}
