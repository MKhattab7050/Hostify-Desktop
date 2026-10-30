use std::{
    net::{Ipv4Addr, TcpListener},
    path::PathBuf,
};
use thiserror::Error;
#[derive(Debug)]
pub struct Site {
    pub verify_root: PathBuf,
    pub port: u16,
    pub backend: Backend,
    pub log: PathBuf,
}
#[derive(Debug)]
pub enum Backend {
    Static { root: PathBuf },
    Proxy { port: u16 },
}
#[derive(Debug, Error)]
pub enum CaddyError {
    #[error("No available local port")]
    NoPort,
    #[error("Path contains characters unsafe in a Caddyfile")]
    UnsafePath,
}
fn quote(path: &std::path::Path) -> Result<String, CaddyError> {
    let s = path.to_string_lossy().replace('\\', "/");
    if s.chars()
        .any(|c| c == '"' || c == '\n' || c == '\r' || c == '{' || c == '}')
    {
        return Err(CaddyError::UnsafePath);
    }
    Ok(format!("\"{s}\""))
}
pub fn generate(sites: &[Site]) -> Result<String, CaddyError> {
    let mut result = String::from("{\n    admin off\n}\n");
    for site in sites {
        // No hostname in the address: tunnel requests carry the public Host, and Caddy answers
        // 200 with an empty body when no site matches. `bind` keeps the listener on loopback.
        match &site.backend {
            Backend::Static { root } => {
                result.push_str(&format!("\nhttp://:{} {{\n    bind 127.0.0.1\n    route {{\n        handle_path /.well-known/hostify-verify/* {{\n            root * {}\n            header Cache-Control no-store\n            file_server\n        }}\n        handle {{\n            root * {}\n            @hidden path_regexp hidden (^|/)\\.\n            respond @hidden 404\n            header {{\n                X-Hostify-Verify hostify\n                X-Content-Type-Options nosniff\n                X-Frame-Options DENY\n                Referrer-Policy strict-origin-when-cross-origin\n            }}\n            file_server\n        }}\n    }}\n",site.port,quote(&site.verify_root)?,quote(root)?));
            }
            Backend::Proxy { port } => {
                result.push_str(&format!("\nhttp://:{} {{\n    bind 127.0.0.1\n    route {{\n        handle_path /.well-known/hostify-verify/* {{\n            root * {}\n            header Cache-Control no-store\n            file_server\n        }}\n        handle /.well-known/hostify-caddy-ok {{\n            respond 200\n        }}\n        handle {{\n            header {{\n                X-Hostify-Verify hostify\n                X-Content-Type-Options nosniff\n                X-Frame-Options DENY\n                Referrer-Policy strict-origin-when-cross-origin\n            }}\n            reverse_proxy 127.0.0.1:{} {{\n                flush_interval -1\n            }}\n        }}\n    }}\n", site.port, quote(&site.verify_root)?, port));
            }
        }
        result.push_str(&format!(
            "    log {{\n        output file {}\n    }}\n}}\n",
            quote(&site.log)?
        ));
    }
    Ok(result)
}
/// Returns a currently bindable port. Callers must still handle a race at actual server startup.
pub fn allocate_port(start: u16, reserved: &[u16]) -> Result<u16, CaddyError> {
    (start.max(1)..=u16::MAX)
        .find(|port| {
            !reserved.contains(port) && TcpListener::bind((Ipv4Addr::LOCALHOST, *port)).is_ok()
        })
        .ok_or(CaddyError::NoPort)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn config_is_loopback_and_static() {
        let s = generate(&[Site {
            verify_root: "verify".into(),
            port: 9000,
            backend: Backend::Static {
                root: "site".into(),
            },
            log: "log".into(),
        }])
        .expect("config");
        assert!(s.contains("admin off"));
        assert!(s.contains("http://:9000 {"));
        assert!(s.contains("bind 127.0.0.1"));
        assert!(s.contains("respond @hidden 404"));
        assert!(s.contains("X-Hostify-Verify hostify"));
        assert!(!s.contains("browse"));
    }
    #[test]
    fn does_not_return_ephemeral_port_zero() {
        assert_ne!(allocate_port(0, &[]).expect("port"), 0);
    }
    #[test]
    fn skips_busy_port() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let port = listener.local_addr().expect("address").port();
        if port < u16::MAX {
            assert_ne!(allocate_port(port, &[]).expect("port"), port);
        }
    }
}

#[cfg(test)]
mod marker_integration {
    use super::*;
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    #[tokio::test]
    async fn real_caddy_marker_and_hidden_paths() {
        let Some(local) = std::env::var_os("LOCALAPPDATA") else {
            println!("SKIP real Caddy: LOCALAPPDATA is unavailable");
            return;
        };
        let binary = PathBuf::from(local).join("Hostify/bin/caddy.exe");
        if !binary.is_file() {
            println!("SKIP real Caddy: {} does not exist", binary.display());
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("site/current");
        let marker = dir.path().join("verify");
        std::fs::create_dir_all(root.join(".well-known")).unwrap();
        std::fs::create_dir_all(&marker).unwrap();
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        std::fs::write(marker.join(&nonce), &nonce).unwrap();
        std::fs::write(root.join(".env"), "secret").unwrap();
        std::fs::write(root.join(".well-known/anything-else"), "secret").unwrap();
        std::fs::write(root.join("index.html"), "website").unwrap();
        let port = allocate_port(19000, &[]).unwrap();
        let config = generate(&[Site {
            verify_root: marker,
            port,
            backend: Backend::Static { root },
            log: dir.path().join("access.log"),
        }])
        .unwrap();
        assert!(config.contains("handle_path /.well-known/hostify-verify/*"));
        assert!(!config.contains("browse"));
        let path = dir.path().join("Caddyfile");
        std::fs::write(&path, config).unwrap();
        let output = std::process::Command::new(&binary)
            .args(["validate", "--adapter", "caddyfile", "--config"])
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let child = std::process::Command::new(&binary)
            .args(["run", "--adapter", "caddyfile", "--config"])
            .arg(&path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let _child = Child(child);
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(2))
            .build()
            .unwrap();
        let base = format!("http://127.0.0.1:{port}");
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                if client.get(&base).send().await.is_ok() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap();
        let response = client
            .get(format!("{base}/.well-known/hostify-verify/{nonce}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["Cache-Control"], "no-store");
        assert_eq!(response.text().await.unwrap(), nonce);
        // The tunnel forwards the public hostname; a Host-bound site returns 200 with no body.
        for (path, body) in [
            ("/".to_string(), "website"),
            (
                format!("/.well-known/hostify-verify/{nonce}"),
                nonce.as_str(),
            ),
        ] {
            let response = client
                .get(format!("{base}{path}"))
                .header(reqwest::header::HOST, "www.example.test")
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200, "{path}");
            assert_eq!(response.text().await.unwrap(), body, "{path}");
        }
        assert!(
            std::net::TcpStream::connect_timeout(
                &std::net::SocketAddr::from(([127, 0, 0, 2], port)),
                std::time::Duration::from_millis(500)
            )
            .is_err(),
            "the listener must stay on 127.0.0.1"
        );
        for path in [
            "/.env",
            "/.well-known/anything-else",
            "/.well-known/hostify-verify/",
        ] {
            assert_eq!(
                client
                    .get(format!("{base}{path}"))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                404,
                "{path}"
            );
        }
        let source = dir.path().join("source");
        std::fs::create_dir(&source).unwrap();
        for i in 0..5 {
            let text = format!("updated {i}");
            std::fs::write(source.join("index.html"), &text).unwrap();
            crate::import::import(&source, "directory", &dir.path().join("site")).unwrap();
            assert_eq!(
                client
                    .get(&base)
                    .send()
                    .await
                    .unwrap()
                    .text()
                    .await
                    .unwrap(),
                text
            );
        }
        assert_eq!(
            std::fs::read_dir(dir.path().join("site/releases"))
                .unwrap()
                .count(),
            3
        );
        println!(
            "Real Caddy validated and served the marker; hidden paths and directory listing returned 404"
        );
    }
}

#[cfg(test)]
mod compatibility_tests {
    use super::*;
    #[test]
    fn static_bytes_match_v1_literal() {
        assert_eq!(
            generate(&[Site {
                verify_root: "verify".into(),
                port: 9000,
                backend: Backend::Static {
                    root: "site".into()
                },
                log: "log".into()
            }])
            .unwrap(),
            r#"{
    admin off
}

http://:9000 {
    bind 127.0.0.1
    route {
        handle_path /.well-known/hostify-verify/* {
            root * "verify"
            header Cache-Control no-store
            file_server
        }
        handle {
            root * "site"
            @hidden path_regexp hidden (^|/)\.
            respond @hidden 404
            header {
                X-Hostify-Verify hostify
                X-Content-Type-Options nosniff
                X-Frame-Options DENY
                Referrer-Policy strict-origin-when-cross-origin
            }
            file_server
        }
    }
    log {
        output file "log"
    }
}
"#
        );
    }
    #[test]
    fn proxy_keeps_marker_headers_and_independent_health() {
        let c = generate(&[Site {
            verify_root: "verify".into(),
            port: 9000,
            backend: Backend::Proxy { port: 9001 },
            log: "log".into(),
        }])
        .unwrap();
        for text in [
            "handle_path /.well-known/hostify-verify/*",
            "handle /.well-known/hostify-caddy-ok",
            "reverse_proxy 127.0.0.1:9001",
            "flush_interval -1",
            "X-Hostify-Verify hostify",
        ] {
            assert!(c.contains(text));
        }
        assert!(!c.contains("file_server\n        }\n    }"));
    }
}
