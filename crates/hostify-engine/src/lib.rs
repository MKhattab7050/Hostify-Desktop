//! Headless Hostify engine and its per-user control protocol.
#[cfg(windows)]
pub mod cli;
#[cfg(windows)]
pub mod pipe;
pub mod state;
pub mod supervisor;
#[cfg(windows)]
pub mod windows;
use serde::Deserialize;
use serde_json::Value;
use std::{
    net::{Ipv4Addr, TcpListener},
    time::Duration,
};
pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub id: u64,
    pub method: String,
    pub params: Value,
}
pub fn validate_request(bytes: &[u8]) -> Result<Request, String> {
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err("Request exceeds the 1 MiB limit".into());
    }
    let r: Request = serde_json::from_slice(bytes)
        .map_err(|_| "Invalid request: expected id (u64), method and params object".to_owned())?;
    if r.method.is_empty() || !r.params.is_object() {
        return Err("Method must be nonempty and params must be an object".into());
    }
    Ok(r)
}
pub fn restart_delay(failures: u32) -> Duration {
    Duration::from_secs(
        1u64.checked_shl(failures.saturating_sub(1))
            .unwrap_or(u64::MAX)
            .min(60),
    )
}
pub fn port_available(port: u16) -> bool {
    port != 0 && TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn backoff() {
        for (n, s) in [(0, 1), (1, 1), (2, 2), (6, 32), (7, 60), (u32::MAX, 60)] {
            assert_eq!(restart_delay(n).as_secs(), s)
        }
    }
    #[test]
    fn validation() {
        assert!(validate_request(br#"{"id":1,"method":"ping","params":{}}"#).is_ok());
        for b in [
            br#"{"id":-1,"method":"ping","params":{}}"#.as_slice(),
            br#"{"id":1,"method":"ping","params":[]}"#,
            br#"{"id":"a","method":"ping","params":{}}"#,
            br#"{"id":1,"method":"","params":{}}"#,
            br#"{"id":1,"method":"ping"}"#,
        ] {
            assert!(validate_request(b).is_err())
        }
        assert!(validate_request(&vec![0; MAX_REQUEST_BYTES + 1]).is_err());
    }
    #[test]
    fn port_conflict() {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let p = l.local_addr().expect("addr").port();
        assert!(!port_available(p));
        assert!(!port_available(0));
        drop(l);
        assert!(port_available(p));
    }
}

pub mod build;
