use std::{net::{Ipv4Addr, TcpListener}, path::PathBuf};
use thiserror::Error;
#[derive(Debug)] pub struct Site { pub port: u16, pub root: PathBuf, pub log: PathBuf }
#[derive(Debug, Error)] pub enum CaddyError { #[error("No available local port") ] NoPort, #[error("Path contains characters unsafe in a Caddyfile") ] UnsafePath }
fn quote(path: &std::path::Path) -> Result<String, CaddyError> {
    let s = path.to_string_lossy().replace('\\', "/");
    if s.chars().any(|c| c == '"' || c == '\n' || c == '\r' || c == '{' || c == '}') { return Err(CaddyError::UnsafePath); }
    Ok(format!("\"{s}\""))
}
pub fn generate(sites: &[Site]) -> Result<String, CaddyError> {
    let mut result = String::from("{\n    admin off\n}\n");
    for site in sites {
        result.push_str(&format!("\nhttp://127.0.0.1:{} {{\n    root * {}\n    @hidden path_regexp hidden (^|/)\\.\n", site.port, quote(&site.root)?));
        result.push_str("    respond @hidden 404\n    header {\n        X-Content-Type-Options nosniff\n        X-Frame-Options DENY\n        Referrer-Policy strict-origin-when-cross-origin\n    }\n    file_server\n");
        result.push_str(&format!("    log {{\n        output file {}\n    }}\n}}\n", quote(&site.log)?));
    }
    Ok(result)
}
/// Returns a currently bindable port. Callers must still handle a race at actual server startup.
pub fn allocate_port(start: u16, reserved: &[u16]) -> Result<u16, CaddyError> {
    (start..=u16::MAX).find(|port| !reserved.contains(port) && TcpListener::bind((Ipv4Addr::LOCALHOST, *port)).is_ok()).ok_or(CaddyError::NoPort)
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn config_is_loopback_and_static() { let s = generate(&[Site { port: 9000, root: "site".into(), log: "log".into() }]).expect("config"); assert!(s.contains("admin off")); assert!(s.contains("127.0.0.1:9000")); assert!(s.contains("respond @hidden 404")); assert!(!s.contains("browse")); }
    #[test] fn skips_busy_port() { let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind"); let port = listener.local_addr().expect("address").port(); if port < u16::MAX { assert_ne!(allocate_port(port, &[]).expect("port"), port); } }
}


