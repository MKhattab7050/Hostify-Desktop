use async_trait::async_trait;
use sha2::{Digest, Sha256, Sha512};
use std::{io::{Cursor, Read}, path::{Path, PathBuf}, sync::Arc};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ComponentError {
    #[error("Download failed: {0}")] Download(String),
    #[error("Missing or invalid checksum for {0}")] Checksum(String),
    #[error("Component archive error: {0}")] Archive(#[from] zip::result::ZipError),
    #[error("Component I/O error: {0}")] Io(#[from] std::io::Error),
}
#[async_trait]
pub trait Http: Send + Sync { async fn get(&self, url: &str) -> Result<Vec<u8>, ComponentError>; }
pub struct ReqwestHttp(pub reqwest::Client);
#[async_trait]
impl Http for ReqwestHttp { async fn get(&self, url: &str) -> Result<Vec<u8>, ComponentError> { let r=self.0.get(url).header("User-Agent","Hostify").send().await.map_err(|e| ComponentError::Download(e.to_string()))?.error_for_status().map_err(|e| ComponentError::Download(e.to_string()))?; Ok(r.bytes().await.map_err(|e| ComponentError::Download(e.to_string()))?.to_vec()) } }
pub struct Components { pub http: Arc<dyn Http>, pub bin: PathBuf, pub github_api: String }
impl Components {
    pub fn new(bin: PathBuf) -> Result<Self, ComponentError> { let client=reqwest::Client::builder().timeout(std::time::Duration::from_secs(120)).build().map_err(|e| ComponentError::Download(e.to_string()))?; Ok(Self { http: Arc::new(ReqwestHttp(client)), bin, github_api:"https://api.github.com".into() }) }
    pub async fn install(&self, name: &str, progress: impl Fn(&str,u8)) -> Result<String, ComponentError> {
        let repo=match name { "caddy"=>"caddyserver/caddy", "cloudflared"=>"cloudflare/cloudflared", _=>return Err(ComponentError::Download("Unknown component".into())) };
        progress(name,0); let release=self.http.get(&format!("{}/repos/{repo}/releases/latest",self.github_api)).await?;
        let release: serde_json::Value=serde_json::from_slice(&release).map_err(|e| ComponentError::Download(e.to_string()))?;
        let version=release["tag_name"].as_str().ok_or_else(|| ComponentError::Download("Missing release version".into()))?;
        let asset=if name=="caddy" { format!("caddy_{}_windows_amd64.zip",version.trim_start_matches('v')) } else { "cloudflared-windows-amd64.exe".into() };
        let assets=release["assets"].as_array().ok_or_else(|| ComponentError::Download("Missing release assets".into()))?;
        let get_url=|n: &str| assets.iter().find(|a| a["name"].as_str()==Some(n)).and_then(|a| a["browser_download_url"].as_str()).map(str::to_owned).ok_or_else(|| ComponentError::Checksum(n.into()));
        // Publisher-provided checksums only. Caddy: checksums.txt (SHA-512, `hash  file`). cloudflared: SHA256 list in the release notes (`file: hash`).
        let listing=if name=="caddy" { let bytes=self.http.get(&get_url(&format!("caddy_{}_checksums.txt",version.trim_start_matches('v')))?).await?; String::from_utf8(bytes).map_err(|_| ComponentError::Checksum(asset.clone()))? } else { release["body"].as_str().unwrap_or("").to_owned() };
        let from_notes=name!="caddy";
        let expected=listing.lines().find_map(|line| { let line=line.trim(); let (file,hash)=if from_notes { let (f,h)=line.split_once(':')?; (f.trim(),h.trim()) } else { let mut p=line.split_whitespace(); let h=p.next()?; let f=p.next()?; (f.trim_start_matches('*'),h) }; if file==asset && (hash.len()==64 || hash.len()==128) && hash.bytes().all(|b| b.is_ascii_hexdigit()) { Some(hash.to_ascii_lowercase()) } else { None } }).ok_or_else(|| ComponentError::Checksum(asset.clone()))?;
        progress(name,20); let bytes=self.http.get(&get_url(&asset)?).await?; let actual=if expected.len()==128 { hex::encode(Sha512::digest(&bytes)) } else { hex::encode(Sha256::digest(&bytes)) }; if actual!=expected { return Err(ComponentError::Checksum(asset)); } progress(name,80);
        let executable=if name=="caddy" { let mut archive=zip::ZipArchive::new(Cursor::new(bytes))?; let mut file=archive.by_name("caddy.exe")?; let mut buf=Vec::new(); file.read_to_end(&mut buf)?; buf } else { bytes };
        std::fs::create_dir_all(&self.bin)?; let tmp=self.bin.join(format!(".{name}-{}.tmp",uuid::Uuid::new_v4())); std::fs::write(&tmp,executable)?;
        let target=self.bin.join(format!("{name}.exe")); if let Err(e)=replace(&tmp,&target) { let _=std::fs::remove_file(&tmp); return Err(e.into()); } progress(name,100); Ok(version.to_owned())
    }
    pub async fn version(&self, name: &str) -> Result<String, ComponentError> { let output=tokio::process::Command::new(self.bin.join(format!("{name}.exe"))).arg("--version").output().await?; if !output.status.success() { return Err(ComponentError::Download("Version command failed".into())); } Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned()) }
}
fn replace(tmp: &Path, target: &Path) -> std::io::Result<()> { std::fs::rename(tmp,target) }
#[cfg(test)] mod tests {
 use super::*; use std::collections::HashMap;
 struct MockHttp(HashMap<String,Vec<u8>>);
 #[async_trait] impl Http for MockHttp { async fn get(&self,url:&str)->Result<Vec<u8>,ComponentError> { self.0.get(url).cloned().ok_or_else(||ComponentError::Download("missing mock".into())) } }
 #[tokio::test] async fn refuses_bad_checksum_without_installing() {
  let dir=tempfile::tempdir().expect("dir"); let mut m=HashMap::new();
  let body = format!("### SHA256 Checksums:
```
cloudflared-windows-amd64.exe: {}
```", "0".repeat(64));
  let release = serde_json::json!({"tag_name":"v1","assets":[{"name":"cloudflared-windows-amd64.exe","browser_download_url":"exe"}],"body":body});
  m.insert("mock/repos/cloudflare/cloudflared/releases/latest".into(), release.to_string().into_bytes());
  m.insert("exe".into(),b"wrong".to_vec());
  let c=Components{http:Arc::new(MockHttp(m)),bin:dir.path().into(),github_api:"mock".into()}; assert!(matches!(c.install("cloudflared",|_,_|{}).await,Err(ComponentError::Checksum(_)))); assert!(!dir.path().join("cloudflared.exe").exists());
 }
}
