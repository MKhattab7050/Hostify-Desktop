use std::{env, ffi::OsString, path::PathBuf};

pub const CADDY_EXE: &str = "caddy.exe";
pub const CLOUDFLARED_EXE: &str = "cloudflared.exe";
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PathError {
    #[error("--data-dir requires a path")]
    MissingArgument,
    #[error("LOCALAPPDATA is not set")]
    MissingLocalAppData,
    #[error("Could not create Hostify data directory: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct HostifyPaths { pub root: PathBuf, pub db: PathBuf, pub sites: PathBuf, pub bin: PathBuf, pub logs: PathBuf, pub run: PathBuf }
impl HostifyPaths {
    pub fn discover() -> Result<Self, PathError> {
        Self::resolve(&env::args_os().collect::<Vec<_>>(), env::var_os("HOSTIFY_DATA_DIR"), env::var_os("LOCALAPPDATA"))
    }
    /// Pure resolution: explicit CLI root wins over environment and the user default.
    pub fn resolve(args: &[OsString], configured: Option<OsString>, local: Option<OsString>) -> Result<Self, PathError> {
        let root = if let Some(i) = args.iter().position(|a| a == "--data-dir") {
            let value = args.get(i + 1).filter(|v| !v.is_empty() && !v.to_string_lossy().starts_with("--")).ok_or(PathError::MissingArgument)?;
            PathBuf::from(value)
        } else if let Some(value) = configured { PathBuf::from(value) }
        else { PathBuf::from(local.ok_or(PathError::MissingLocalAppData)?).join("Hostify") };
        Ok(Self { db: root.join("hostify.db"), sites: root.join("sites"), bin: root.join("bin"), logs: root.join("logs"), run: root.join("run"), root })
    }
    pub fn create(&self) -> Result<(), PathError> {
        for path in [&self.root, &self.sites, &self.bin, &self.logs, &self.run] { std::fs::create_dir_all(path)?; }
        Ok(())
    }
}

#[cfg(test)] mod tests {
 use super::*;
 #[test] fn resolution_precedence_and_bin() {
  let args = vec!["engine".into(), "--data-dir".into(), "explicit".into()];
  let p = HostifyPaths::resolve(&args, Some("env".into()), Some("local".into())).unwrap();
  assert_eq!(p.root, PathBuf::from("explicit")); assert_eq!(p.bin, p.root.join("bin"));
  assert_eq!(HostifyPaths::resolve(&[], Some("env".into()), None).unwrap().root, PathBuf::from("env"));
  assert_eq!(HostifyPaths::resolve(&[], None, Some("local".into())).unwrap().root, PathBuf::from("local").join("Hostify"));
  assert!(HostifyPaths::resolve(&["--data-dir".into()], None, None).is_err());
  assert!(HostifyPaths::resolve(&[], None, None).is_err());
 }
}
