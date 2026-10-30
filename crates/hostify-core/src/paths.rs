use std::{env, path::PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PathError {
    #[error("LOCALAPPDATA is not set")]
    MissingLocalAppData,
    #[error("Could not create Hostify data directory: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct HostifyPaths { pub root: PathBuf, pub db: PathBuf, pub sites: PathBuf, pub bin: PathBuf, pub logs: PathBuf, pub run: PathBuf }
impl HostifyPaths {
    pub fn discover() -> Result<Self, PathError> {
        let root = PathBuf::from(env::var_os("LOCALAPPDATA").ok_or(PathError::MissingLocalAppData)?).join("Hostify");
        Ok(Self { db: root.join("hostify.db"), sites: root.join("sites"), bin: root.join("bin"), logs: root.join("logs"), run: root.join("run"), root })
    }
    pub fn create(&self) -> Result<(), PathError> {
        for path in [&self.root, &self.sites, &self.bin, &self.logs, &self.run] { std::fs::create_dir_all(path)?; }
        Ok(())
    }
}
