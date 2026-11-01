use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Install {
    Ci,
    #[default]
    Install,
    None,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RestartPolicy {
    #[default]
    Always,
    OnFailure,
    Never,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Start {
    NpmScript {
        #[serde(default)]
        name: String,
    },
    Node {
        #[serde(default)]
        args: Vec<String>,
    },
}
impl Default for Start {
    fn default() -> Self {
        Self::NpmScript {
            name: "start".into(),
        }
    }
}
fn dot() -> String {
    ".".into()
}
fn slash() -> String {
    "/".into()
}
fn workers() -> u8 {
    4
}
fn extensions() -> Vec<String> {
    [
        "mbstring",
        "openssl",
        "curl",
        "fileinfo",
        "mysqli",
        "pdo_mysql",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}
pub const PHP_EXTENSIONS: &[&str] = &[
    "mbstring",
    "openssl",
    "curl",
    "fileinfo",
    "mysqli",
    "pdo_mysql",
    "gd",
    "intl",
    "exif",
    "zip",
    "soap",
    "sodium",
    "pdo_sqlite",
    "sqlite3",
];
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    #[serde(default = "dot")]
    pub document_root: String,
    #[serde(default = "workers")]
    pub php_workers: u8,
    #[serde(default = "extensions")]
    pub php_extensions: Vec<String>,
    #[serde(default)]
    pub php_settings: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub persistent_paths: Vec<String>,
    #[serde(default)]
    pub php_ports: Vec<u16>,
    #[serde(default)]
    pub install: Install,
    #[serde(default)]
    pub build_script: Option<String>,
    #[serde(default)]
    pub start: Start,
    #[serde(default = "dot")]
    pub working_dir: String,
    #[serde(default = "slash")]
    pub health_path: String,
    #[serde(default)]
    pub restart: RestartPolicy,
    #[serde(default)]
    pub memory_limit_mb: Option<u64>,
    #[serde(default)]
    pub allow_install_scripts: bool,
    #[serde(default)]
    pub internal_port: Option<u16>,
    #[serde(default)]
    pub active_release: Option<String>,
    #[serde(default)]
    pub previous_release: Option<String>,
    #[serde(default)]
    pub pending_release: Option<String>,
    #[serde(default)]
    pub built_release: Option<String>,
    #[serde(default)]
    pub trusted_release: Option<String>,
}
impl Default for AppConfig {
    fn default() -> Self {
        serde_json::from_str("{}").expect("configuration defaults")
    }
}
/// Lexically validate on every platform, including Windows spellings on Unix.
pub fn relative(text: &str) -> Result<PathBuf, String> {
    if text == "." {
        return Ok(PathBuf::from("."));
    }
    if text.is_empty()
        || text.starts_with(['/', '\\'])
        || text.contains([':', '\0'])
        || text.split(['/', '\\']).any(|c| c == "..")
    {
        return Err("Paths must be relative to the release and cannot contain parent traversal, drive prefixes, UNC prefixes or NUL.".into());
    }
    let clean: PathBuf = text
        .split(['/', '\\'])
        .filter(|c| !c.is_empty() && *c != ".")
        .collect();
    if clean.as_os_str().is_empty() {
        Ok(PathBuf::from("."))
    } else {
        Ok(clean)
    }
}
pub fn inside(release: &Path, text: &str) -> Result<PathBuf, String> {
    let rel = relative(text)?;
    let root = release
        .canonicalize()
        .map_err(|e| format!("Cannot open release directory: {e}"))?;
    let path = root.join(&rel);
    let canonical = match path.canonicalize() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!(
                "The configured path '{text}' does not exist inside the project files (checked '{}').",
                path.display()
            ));
        }
        Err(e) => return Err(format!("Cannot open the configured path '{text}': {e}")),
    };
    if !canonical.starts_with(&root) {
        return Err(
            "The configured path must stay inside the release, including through junctions.".into(),
        );
    }
    Ok(canonical)
}

pub fn find_best_node_sub(root: &Path) -> Option<String> {
    if root.join("package.json").is_file() {
        return Some(".".into());
    }

    let entries = std::fs::read_dir(root).ok()?;
    let mut best: Option<(i32, String)> = None;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }

        let pkg_path = path.join("package.json");
        if pkg_path.is_file() {
            let mut score = 1;
            if path.join("server.js").is_file()
                || path.join("app.js").is_file()
                || path.join("index.js").is_file()
            {
                score += 10;
            }
            if path.join(".env").is_file() {
                score += 5;
            }
            if let Ok(bytes) = std::fs::read(&pkg_path)
                && let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes)
                && v["scripts"]["start"].is_string()
            {
                score += 10;
            }
            if best.as_ref().is_none_or(|(b, _)| score > *b) {
                best = Some((score, name.to_owned()));
            }
        } else if path.join("server.js").is_file()
            || path.join("app.js").is_file()
            || path.join("index.js").is_file()
        {
            let score = 5;
            if best.as_ref().is_none_or(|(b, _)| score > *b) {
                best = Some((score, name.to_owned()));
            }
        } else if let Ok(sub_entries) = std::fs::read_dir(&path) {
            for sub_entry in sub_entries.flatten() {
                let sub_path = sub_entry.path();
                if !sub_path.is_dir() {
                    continue;
                }
                let Some(sub_name) = sub_path.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                if sub_name.starts_with('.') || sub_name == "node_modules" {
                    continue;
                }
                if sub_path.join("package.json").is_file() || sub_path.join("server.js").is_file() {
                    let score = 3;
                    let relative_sub = format!("{name}/{sub_name}");
                    if best.as_ref().is_none_or(|(b, _)| score > *b) {
                        best = Some((score, relative_sub));
                    }
                }
            }
        }
    }

    best.map(|(_, d)| d)
}

pub fn find_best_php_sub(root: &Path) -> Option<String> {
    if root.join("public/index.php").is_file() {
        return Some("public".into());
    }
    if root.join("index.php").is_file() {
        return Some(".".into());
    }

    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if name.starts_with('.') || name == "vendor" {
            continue;
        }
        if path.join("public/index.php").is_file() {
            return Some(format!("{name}/public"));
        }
        if path.join("index.php").is_file() {
            return Some(name.to_owned());
        }
    }
    None
}
impl AppConfig {
    pub fn validate(&self, release: &Path) -> Result<(), String> {
        self.validate_kind(release, "node")
    }
    pub fn validate_php(&self) -> Result<(), String> {
        if !(1..=16).contains(&self.php_workers) {
            return Err("PHP workers must be between 1 and 16.".into());
        }
        for name in &self.php_extensions {
            if !PHP_EXTENSIONS.contains(&name.as_str()) {
                return Err(format!("PHP extension {name} is not allowed."));
            }
        }
        for (key, value) in &self.php_settings {
            let seconds = key == "max_execution_time";
            if !seconds
                && !["memory_limit", "upload_max_filesize", "post_max_size"].contains(&key.as_str())
            {
                return Err("PHP setting is not allowed.".into());
            }
            let digits = if !seconds {
                value.trim_end_matches(['K', 'M', 'G'])
            } else {
                value
            };
            if digits.is_empty()
                || value.len() > digits.len() + 1
                || !digits.bytes().all(|b| b.is_ascii_digit())
            {
                return Err(format!("Invalid PHP setting {key}."));
            }
        }
        Ok(())
    }
    pub fn auto_heal(&mut self, release: &Path, kind: &str) -> bool {
        let mut changed = false;
        let root = match release.canonicalize() {
            Ok(r) => r,
            Err(_) => return false,
        };

        if kind == "node" {
            let cwd_ok = inside(&root, &self.working_dir)
                .map(|p| {
                    p.join("package.json").is_file()
                        || p.join("server.js").is_file()
                        || p.join("app.js").is_file()
                })
                .unwrap_or(false);

            if !cwd_ok {
                if let Some(sub) = find_best_node_sub(&root) {
                    if self.working_dir != sub {
                        self.working_dir = sub.clone();
                        changed = true;
                    }
                    if self.document_root == "."
                        || self.document_root.is_empty()
                        || !root.join(&self.document_root).is_dir()
                    {
                        self.document_root = sub;
                        changed = true;
                    }
                }
            }

            if let Start::Node { ref mut args } = self.start {
                if let Some(entry_arg) = args.first_mut() {
                    let cwd = inside(&root, &self.working_dir).unwrap_or_else(|_| root.clone());
                    if !cwd.join(&*entry_arg).is_file() {
                        let prefix = format!("{}/", self.working_dir);
                        if let Some(stripped) = entry_arg.strip_prefix(&prefix) {
                            if cwd.join(stripped).is_file() {
                                *entry_arg = stripped.to_string();
                                changed = true;
                            }
                        } else if let Some(file_name) =
                            Path::new(&*entry_arg).file_name().and_then(|s| s.to_str())
                        {
                            if cwd.join(file_name).is_file() {
                                *entry_arg = file_name.to_string();
                                changed = true;
                            } else if root.join(file_name).is_file() {
                                if self.working_dir != "." {
                                    self.working_dir = ".".into();
                                    *entry_arg = file_name.to_string();
                                    changed = true;
                                }
                            }
                        }
                    }
                }
            }
        } else if kind == "php" {
            let doc_ok = inside(&root, &self.document_root)
                .map(|p| p.join("index.php").is_file())
                .unwrap_or(false);

            if !doc_ok {
                if let Some(sub) = find_best_php_sub(&root) {
                    if self.document_root != sub {
                        self.document_root = sub;
                        changed = true;
                    }
                }
            }
        }
        changed
    }

    pub fn validate_kind(&self, release: &Path, kind: &str) -> Result<(), String> {
        self.validate_php()?;
        let doc_dir = inside(release, &self.document_root)
            .map_err(|e| format!("Website folder (documentRoot): {e}"))?;
        if !doc_dir.is_dir() {
            return Err("The document root must be a directory.".into());
        }
        for text in &self.persistent_paths {
            let candidate = release.join(relative(text)?);
            let ancestor = candidate
                .ancestors()
                .find(|p| p.exists())
                .ok_or("Cannot resolve persistent path.")?;
            if !ancestor
                .canonicalize()
                .map_err(|e| e.to_string())?
                .starts_with(release.canonicalize().map_err(|e| e.to_string())?)
            {
                return Err(
                    "Persistent paths must stay inside the release, including through junctions."
                        .into(),
                );
            }
        }
        let cwd = inside(release, &self.working_dir)
            .map_err(|e| format!("Working folder (workingDir): {e}"))?;
        if !cwd.is_dir() {
            return Err("The working directory must be a directory.".into());
        }
        if !self.health_path.starts_with('/')
            || self
                .health_path
                .chars()
                .any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(
                "The health path must start with / and contain no spaces or control characters."
                    .into(),
            );
        }
        if self
            .memory_limit_mb
            .is_some_and(|m| !(128..=65536).contains(&m))
        {
            return Err("The memory limit must be between 128 and 65536 MiB.".into());
        }
        if self.internal_port == Some(0) {
            return Err("The internal port must be greater than zero.".into());
        }
        for id in [
            &self.active_release,
            &self.previous_release,
            &self.pending_release,
            &self.built_release,
            &self.trusted_release,
        ]
        .into_iter()
        .flatten()
        {
            uuid::Uuid::parse_str(id).map_err(|_| "The release ID is invalid.")?;
        }
        if kind == "php" {
            return Ok(());
        }
        let script = |name: &str| -> Result<(), String> {
            let p = cwd.join("package.json");
            let meta =
                std::fs::metadata(&p).map_err(|e| format!("Cannot read package.json: {e}"))?;
            if meta.len() > 2 * 1024 * 1024 {
                return Err("package.json exceeds the 2 MiB limit.".into());
            }
            let package: serde_json::Value =
                serde_json::from_slice(&std::fs::read(p).map_err(|e| e.to_string())?)
                    .map_err(|_| "package.json is not valid JSON.")?;
            if package["scripts"][name].as_str().is_none() {
                return Err(format!("The script {name} does not exist in package.json."));
            }
            Ok(())
        };
        if let Some(name) = &self.build_script {
            script(name)?;
        }
        match &self.start {
            Start::NpmScript { name } => script(name)?,
            Start::Node { args } => {
                if args.is_empty() {
                    return Err("The Node start command needs a relative entry path.".into());
                }
                if args[0].starts_with('-') {
                    return Err(
                        "The Node start command must begin with an entry file, not an option."
                            .into(),
                    );
                }
                for (i, arg) in args.iter().enumerate() {
                    let stripped = arg.replace("{PORT}", "1");
                    if stripped.contains(['{', '}', '\0']) {
                        return Err("Only the {PORT} placeholder is supported.".into());
                    }
                    if i == 0 || arg.contains(['/', '\\', ':']) || arg == ".." {
                        let path = relative(arg)?;
                        // Dependencies may not exist before npm install; validate the nearest existing ancestor too.
                        let candidate = cwd.join(path);
                        let ancestor = candidate
                            .ancestors()
                            .find(|p| p.exists())
                            .ok_or("The configured path cannot be resolved.")?;
                        if !ancestor
                            .canonicalize()
                            .map_err(|e| e.to_string())?
                            .starts_with(release.canonicalize().map_err(|e| e.to_string())?)
                        {
                            return Err("The start path must stay inside the release, including through junctions.".into());
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_and_traversal() {
        let c = AppConfig::default();
        assert_eq!(c.working_dir, ".");
        assert_eq!(c.restart, RestartPolicy::Always);
        for p in ["..\\x", "C:\\x", "\\\\host\\x", "a/../x", "a\0"] {
            assert!(relative(p).is_err(), "{p}");
        }
    }
    #[test]
    fn scripts_health_and_memory() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("package.json"),
            r#"{"scripts":{"start":"node server.js"}}"#,
        )
        .unwrap();
        let mut c = AppConfig::default();
        assert!(c.validate(d.path()).is_ok());
        c.health_path = "bad".into();
        assert!(c.validate(d.path()).is_err());
        c.health_path = "/".into();
        c.memory_limit_mb = Some(127);
        assert!(c.validate(d.path()).is_err());
        c.memory_limit_mb = None;
        c.start = Start::NpmScript {
            name: "missing".into(),
        };
        assert!(c.validate(d.path()).is_err());
        c.start = Start::Node {
            args: vec!["--eval".into(), "code".into()],
        };
        assert!(c.validate(d.path()).unwrap_err().contains("entry file"));
    }
}
#[cfg(all(test, windows))]
mod junction_config_tests {
    use super::*;
    #[test]
    fn canonical_working_directory_and_entry_cannot_escape_junction() {
        let release = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let link = release.path().join("junction");
        let result = std::process::Command::new("cmd.exe")
            .args(["/D", "/C", "mklink", "/J"])
            .arg(&link)
            .arg(outside.path())
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let mut c = AppConfig {
            working_dir: "junction".into(),
            ..Default::default()
        };
        assert!(c.validate(release.path()).unwrap_err().contains("inside"));
        c.working_dir = ".".into();
        c.start = Start::Node {
            args: vec!["junction/server.js".into()],
        };
        assert!(c.validate(release.path()).unwrap_err().contains("inside"));
        std::fs::remove_dir(&link).unwrap();
    }
}

#[cfg(test)]
mod php_tests {
    use super::*;
    #[test]
    fn php_defaults_validation_and_no_package_requirement() {
        let d = tempfile::tempdir().unwrap();
        let mut c = AppConfig::default();
        assert!(c.validate_kind(d.path(), "php").is_ok());
        assert!(c.validate(d.path()).is_err());
        for value in ["", "1MM", "-1", "128m", "1\nx=1"] {
            c.php_settings.insert("memory_limit".into(), value.into());
            assert!(c.validate_php().is_err(), "{value}");
        }
        c.php_settings.insert("memory_limit".into(), "128M".into());
        assert!(c.validate_php().is_ok());
        c.php_settings
            .insert("max_execution_time".into(), "1M".into());
        assert!(c.validate_php().is_err());
        c.php_settings.clear();
        c.php_workers = 17;
        assert!(c.validate_php().is_err());
        c.php_workers = 1;
        c.php_extensions.push("not_allowed".into());
        assert!(c.validate_php().is_err());
        c.php_extensions.pop();
        c.document_root = "../outside".into();
        assert!(c.validate_kind(d.path(), "php").is_err());
        c.document_root = ".".into();
        c.persistent_paths = vec!["../outside".into()];
        assert!(c.validate_kind(d.path(), "php").is_err());
    }
}

#[cfg(test)]
mod auto_heal_tests {
    use super::*;

    #[test]
    fn auto_heals_nested_node_project_and_entry_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let cyphers = dir.path().join("Cyphers");
        std::fs::create_dir_all(&cyphers).unwrap();
        std::fs::write(
            cyphers.join("package.json"),
            r#"{"name":"cyphers-chat","scripts":{"start":"node server.js"}}"#,
        )
        .unwrap();
        std::fs::write(cyphers.join("server.js"), "console.log('hi');").unwrap();

        let mut c = AppConfig {
            working_dir: ".".into(),
            document_root: ".".into(),
            start: Start::Node {
                args: vec!["Cyphers/server.js".into()],
            },
            ..Default::default()
        };

        // Before auto_heal, validating root as working_dir would fail because package.json/server.js are in Cyphers
        let changed = c.auto_heal(dir.path(), "node");
        assert!(changed);
        assert_eq!(c.working_dir, "Cyphers");
        assert_eq!(c.document_root, "Cyphers");
        if let Start::Node { ref args } = c.start {
            assert_eq!(args[0], "server.js");
        } else {
            panic!("Expected Start::Node");
        }

        // Now validate succeeds
        assert!(c.validate(dir.path()).is_ok());
    }

    #[test]
    fn auto_heals_nested_php_project() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("myapp").join("public");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("index.php"), "<?php echo 'ok';").unwrap();

        let mut c = AppConfig {
            document_root: ".".into(),
            ..Default::default()
        };

        let changed = c.auto_heal(dir.path(), "php");
        assert!(changed);
        assert_eq!(c.document_root, "myapp/public");
        assert!(c.validate_kind(dir.path(), "php").is_ok());
    }

    #[test]
    fn clear_error_message_on_missing_configured_path() {
        let dir = tempfile::tempdir().unwrap();
        let err = inside(dir.path(), "non_existent_folder").unwrap_err();
        assert!(err.contains("non_existent_folder"));
        assert!(err.contains("does not exist"));
    }
}
