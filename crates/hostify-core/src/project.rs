//! Passive project inspection: no project commands are executed.
use crate::app_config::{AppConfig, Install, Start};
use crate::import::{is_link, open_regular, safe_name};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
};
const LIMIT: u64 = 2 * 1024 * 1024;
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectReport {
    pub site_kind: String,
    pub framework: Option<String>,
    pub name: Option<String>,
    pub engines: Option<String>,
    pub recommended_runtime: String,
    pub scripts: BTreeMap<String, String>,
    pub lifecycle_scripts: BTreeMap<String, String>,
    pub dependencies_with_install_scripts: Vec<String>,
    pub lockfile: Option<String>,
    pub has_server_entry: bool,
    pub dotenv_names: Vec<String>,
    pub uses_database: bool,
    pub suggested: AppConfig,
    pub warnings: Vec<String>,
}
fn wanted(p: &Path) -> bool {
    p.components().count() <= 2
        && p.file_name().and_then(|s| s.to_str()).is_some_and(|n| {
            matches!(
                n,
                "package.json" | "package-lock.json" | ".env" | "composer.json"
            )
        })
}
fn read_limited(mut f: impl Read, size: u64) -> Result<Vec<u8>, String> {
    if size > LIMIT {
        return Err("A parsed project file exceeds the 2 MiB limit.".into());
    }
    let mut bytes = Vec::new();
    f.by_ref()
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > LIMIT {
        return Err("A parsed project file exceeds the 2 MiB limit.".into());
    }
    Ok(bytes)
}
type ProjectFiles = (BTreeSet<PathBuf>, BTreeMap<PathBuf, Vec<u8>>);
fn files(path: &Path, kind: &str) -> Result<ProjectFiles, String> {
    let mut names = BTreeSet::new();
    let mut data = BTreeMap::new();
    if kind == "directory" {
        if is_link(&fs::symlink_metadata(path).map_err(|e| e.to_string())?) {
            return Err("Project links are not allowed.".into());
        }
        for n in [
            "package.json",
            "package-lock.json",
            "pnpm-lock.yaml",
            "yarn.lock",
            ".env",
            "server.js",
            "app.js",
            "index.js",
            "index.php",
            "public/index.php",
            "wp-config-sample.php",
            "wp-config.php",
            "vendor",
            "storage",
            "database/schema.sql",
            "app/database.php",
            "config/database.php",
            "composer.json",
            "index.html",
            "dist/index.html",
        ] {
            let p = path.join(n);
            for ancestor in p.ancestors().take_while(|p| *p != path) {
                if let Ok(m) = fs::symlink_metadata(ancestor) {
                    if is_link(&m) {
                        return Err("Project links and junctions are not allowed.".into());
                    }
                }
            }
            if let Ok(m) = fs::symlink_metadata(&p) {
                if m.is_file() || matches!(n, "vendor" | "storage") && m.is_dir() {
                    names.insert(PathBuf::from(n));
                    if wanted(Path::new(n)) {
                        data.insert(
                            PathBuf::from(n),
                            read_limited(
                                open_regular(&p, &m).map_err(|e| e.to_string())?,
                                m.len(),
                            )?,
                        );
                    }
                }
            }
        }
    } else if kind == "zip" {
        let m = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        let mut z = zip::ZipArchive::new(open_regular(path, &m).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        if z.len() > 100_000 {
            return Err("Too many archive entries.".into());
        }
        let mut entries = Vec::new();
        let mut top = BTreeSet::new();
        let mut root = false;
        for i in 0..z.len() {
            let f = z.by_index(i).map_err(|e| e.to_string())?;
            let p = safe_name(&f.name().map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            if f.unix_mode()
                .is_some_and(|m| !matches!(m & 0o170000, 0 | 0o100000 | 0o040000))
            {
                return Err("Archive links or special files are not allowed.".into());
            }
            top.insert(p.components().next().unwrap().as_os_str().to_owned());
            root |= p.components().count() == 1 && !f.is_dir();
            entries.push((p, f.is_dir()));
        }
        let flatten = top.len() == 1 && !root;
        let mut seen = BTreeSet::new();
        for (i, (p, dir)) in entries.into_iter().enumerate() {
            let p = if flatten {
                p.components().skip(1).collect()
            } else {
                p
            };
            if p.as_os_str().is_empty() {
                continue;
            }
            if !seen.insert(p.to_string_lossy().to_lowercase()) {
                return Err("Duplicate archive path.".into());
            }
            if dir && p == Path::new("vendor") {
                names.insert(p.clone());
            }
            if !dir {
                names.insert(p.clone());
                if wanted(&p) {
                    let f = z.by_index(i).map_err(|e| e.to_string())?;
                    let size = f.size();
                    data.insert(p, read_limited(f, size)?);
                }
            }
        }
    } else {
        return Err("Choose directory or zip as the source kind.".into());
    }
    Ok((names, data))
}
fn version(s: &str) -> Option<[u32; 3]> {
    let mut v = [0; 3];
    for (i, n) in s.trim_start_matches('v').split('.').enumerate() {
        if i >= 3 {
            return None;
        }
        v[i] = n.parse().ok()?;
    }
    Some(v)
}
fn satisfies(pin: &str, range: &str) -> bool {
    let Some(v) = version(pin) else {
        return false;
    };
    range.split("||").any(|or| {
        or.split_whitespace().all(|part| {
            if matches!(part, "*" | "x" | "X") {
                return true;
            }
            let op = if part.starts_with(">=") {
                ">="
            } else if part.starts_with("<=") {
                "<="
            } else if part.starts_with('>') {
                ">"
            } else if part.starts_with('<') {
                "<"
            } else if part.starts_with('^') {
                "^"
            } else if part.starts_with('~') {
                "~"
            } else {
                "="
            };
            let raw = part.trim_start_matches(['>', '<', '=', '^', '~']);
            let raw = raw.trim_end_matches(".x").trim_end_matches(".*");
            let Some(w) = version(raw) else {
                return false;
            };
            match op {
                ">=" => v >= w,
                "<=" => v <= w,
                ">" => v > w,
                "<" => v < w,
                "^" => v >= w && v[0] == w[0],
                "~" => v >= w && v[..2] == w[..2],
                _ => v[..raw.split('.').count()] == w[..raw.split('.').count()],
            }
        })
    })
}
// Composer's ~8.4 permits 8.5, while ~8.4.0 stops before 8.5.
fn php_satisfies(pin: &str, range: &str) -> bool {
    let pin = pin.split('-').next().unwrap_or(pin);
    let normalized = range.replace(',', " ");
    normalized.split('|').any(|or| {
        !or.trim().is_empty()
            && or.split_whitespace().all(|part| {
                if let Some(raw) = part.strip_prefix('~')
                    && raw.split('.').count() <= 2
                {
                    return version(pin)
                        .zip(version(raw))
                        .is_some_and(|(v, w)| v >= w && v[0] == w[0]);
                }
                satisfies(pin, part)
            })
    })
}
pub fn read_source_dotenv(path: &Path, kind: &str) -> Option<String> {
    let (names, data) = files(path, kind).ok()?;
    if let Some(bytes) = data.get(Path::new(".env")) {
        return String::from_utf8(bytes.clone()).ok();
    }
    let sub = best_nested_subdir(&names, &data)?;
    let bytes = data.get(&Path::new(&sub).join(".env"))?;
    String::from_utf8(bytes.clone()).ok()
}
fn best_nested_subdir(
    names: &BTreeSet<PathBuf>,
    data: &BTreeMap<PathBuf, Vec<u8>>,
) -> Option<String> {
    if names.contains(Path::new("package.json"))
        || names.contains(Path::new("index.php"))
        || names.contains(Path::new("public/index.php"))
        || names.contains(Path::new("composer.json"))
        || names.contains(Path::new("index.html"))
    {
        return None;
    }
    let mut candidates = BTreeSet::new();
    for p in names {
        let mut comps = p.components();
        if let (Some(first), Some(_)) = (comps.next(), comps.next()) {
            if let Some(s) = first.as_os_str().to_str() {
                candidates.insert(s.to_owned());
            }
        }
    }
    let mut best: Option<(i32, String)> = None;
    for dir in candidates {
        let base = Path::new(&dir);
        let has_pkg = names.contains(&base.join("package.json"));
        let has_php = names.contains(&base.join("index.php"))
            || names.contains(&base.join("public/index.php"))
            || names.contains(&base.join("composer.json"));
        let has_html = names.contains(&base.join("index.html"));
        if !has_pkg && !has_php && !has_html {
            continue;
        }
        let mut score = 1;
        if names.contains(&base.join("server.js")) || names.contains(&base.join("app.js")) {
            score += 10;
        }
        if names.contains(&base.join(".env")) {
            score += 5;
        }
        if let Some(raw) = data.get(&base.join("package.json"))
            && let Ok(v) = serde_json::from_slice::<Value>(raw)
            && v["scripts"]["start"].is_string()
        {
            score += 10;
        }
        if best.as_ref().is_none_or(|(b, _)| score > *b) {
            best = Some((score, dir));
        }
    }
    best.map(|(_, d)| d)
}
pub fn inspect(path: &Path, kind: &str) -> Result<ProjectReport, String> {
    let (raw_names, raw_files) = files(path, kind)?;
    let nested_subdir = best_nested_subdir(&raw_names, &raw_files);
    let (names, files) = if let Some(ref sub) = nested_subdir {
        let prefix = Path::new(sub);
        let n = raw_names
            .into_iter()
            .filter_map(|p| p.strip_prefix(prefix).ok().map(Path::to_path_buf))
            .filter(|p| !p.as_os_str().is_empty())
            .collect::<BTreeSet<_>>();
        let f = raw_files
            .into_iter()
            .filter_map(|(p, v)| p.strip_prefix(prefix).ok().map(|sp| (sp.to_path_buf(), v)))
            .collect::<BTreeMap<_, _>>();
        (n, f)
    } else {
        (raw_names, raw_files)
    };
    let has = |s: &str| names.contains(Path::new(s));
    let parse = |s: &str| -> Result<Value, String> {
        match files.get(Path::new(s)) {
            Some(b) => serde_json::from_slice(b).map_err(|_| format!("{s} is not valid JSON.")),
            None => Ok(Value::Null),
        }
    };
    let p = parse("package.json")?;
    let lock = parse("package-lock.json")?;
    let deps = &p["dependencies"];
    let scripts = &p["scripts"];
    // Most specific first: Socket.IO and Next.js projects often depend on Express as well.
    let framework = ["next", "socket.io", "fastify", "koa", "express"]
        .into_iter()
        .find(|n| deps[*n].is_string())
        .map(str::to_owned);
    let entry = ["server.js", "app.js", "index.js"]
        .into_iter()
        .find(|n| has(n));
    let main = p["main"].as_str();
    let node = !has("wp-config-sample.php")
        && (scripts["start"].is_string()
            || main.is_some()
            || entry.is_some()
            || deps["next"].is_string());
    let mut r = ProjectReport {
        site_kind: if node {
            "node"
        } else if has("index.php")
            || has("public/index.php")
            || has("composer.json")
            || has("wp-config-sample.php")
        {
            "php"
        } else if has("index.html") || (has("package.json") && has("dist/index.html")) {
            "static"
        } else {
            "unknown"
        }
        .into(),
        framework,
        name: p["name"].as_str().map(str::to_owned),
        engines: p["engines"]["node"].as_str().map(str::to_owned),
        recommended_runtime: String::new(),
        scripts: BTreeMap::new(),
        lifecycle_scripts: BTreeMap::new(),
        dependencies_with_install_scripts: Vec::new(),
        lockfile: if has("package-lock.json") {
            Some("package-lock")
        } else if has("pnpm-lock.yaml") {
            Some("pnpm")
        } else if has("yarn.lock") {
            Some("yarn")
        } else {
            None
        }
        .map(str::to_owned),
        has_server_entry: main.is_some() || entry.is_some(),
        dotenv_names: Vec::new(),
        uses_database: ["mysql", "mysql2", "prisma", "sequelize", "knex"]
            .iter()
            .any(|n| deps[*n].is_string()),
        suggested: AppConfig::default(),
        warnings: Vec::new(),
    };
    for n in ["build", "start"] {
        if let Some(s) = scripts[n].as_str() {
            r.scripts.insert(n.into(), s.into());
        }
    }
    for n in ["preinstall", "install", "postinstall", "prepare"] {
        if let Some(s) = scripts[n].as_str() {
            r.lifecycle_scripts.insert(n.into(), s.into());
        }
    }
    if let Some(packages) = lock["packages"].as_object() {
        for (path, p) in packages {
            if p["hasInstallScript"] == true {
                let name = p["name"]
                    .as_str()
                    .unwrap_or_else(|| path.rsplit("node_modules/").next().unwrap_or(path));
                if !name.is_empty() {
                    r.dependencies_with_install_scripts.push(name.into());
                }
            }
        }
    }
    r.dependencies_with_install_scripts.sort();
    r.dependencies_with_install_scripts.dedup();
    if let Some(env) = files.get(Path::new(".env")) {
        for line in String::from_utf8_lossy(env).lines() {
            let line = line.trim().strip_prefix("export ").unwrap_or(line.trim());
            if let Some((name, _)) = line.split_once('=') {
                let name = name.trim();
                if !name.is_empty()
                    && name.bytes().enumerate().all(|(i, b)| {
                        b == b'_' || b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit())
                    })
                {
                    r.dotenv_names.push(name.into());
                }
            }
        }
        r.dotenv_names.sort();
        r.dotenv_names.dedup();
    }
    if r.site_kind == "php" {
        let composer = parse("composer.json")?;
        r.engines = composer["require"]["php"].as_str().map(str::to_owned);
        let pins = crate::runtimes::pinned_versions("php");
        r.recommended_runtime = pins
            .iter()
            .copied()
            .find(|v| r.engines.as_ref().is_none_or(|e| php_satisfies(v, e)))
            .unwrap_or(pins[0])
            .into();
        if r.engines
            .as_ref()
            .is_some_and(|range| !php_satisfies(&r.recommended_runtime, range))
        {
            r.warnings.push("No pinned PHP runtime satisfies require.php; the newest pinned runtime is suggested.".into());
        }
        r.suggested.install = Install::None;
        if has("public/index.php") {
            r.suggested.document_root = "public".into();
        }
        if has("wp-config-sample.php") {
            r.framework = Some("wordpress".into());
            r.suggested.persistent_paths = vec!["wp-content".into(), "wp-config.php".into()];
        } else if names
            .iter()
            .any(|p| p == Path::new("storage") || p.starts_with("storage/"))
        {
            r.suggested.persistent_paths = vec!["storage".into()];
        }
        r.uses_database = has("wp-config-sample.php")
            || has("database/schema.sql")
            || has("app/database.php")
            || has("config/database.php")
            || r.dotenv_names.iter().any(|n| {
                matches!(
                    n.as_str(),
                    "DB_HOST"
                        | "DB_DATABASE"
                        | "DB_NAME"
                        | "DB_USERNAME"
                        | "DB_USER"
                        | "DATABASE_URL"
                )
            })
            || ["require", "require-dev"].iter().any(|section| {
                ["ext-mysqli", "ext-pdo_mysql", "mysqli", "pdo_mysql"]
                    .iter()
                    .any(|k| composer[section][k].is_string())
            });
        if ["require", "require-dev"].iter().any(|section| {
            composer[section]
                .as_object()
                .is_some_and(|d| d.keys().any(|k| k != "php" && !k.starts_with("ext-")))
        }) && !names.iter().any(|p| p.starts_with("vendor"))
        {
            r.warnings.push("Composer dependencies are present but vendor is missing. Run composer install yourself; Hostify does not run it.".into());
        }
        return Ok(r);
    }
    // Only pinned LTS versions are candidates, newest first.
    let pins = ["26.13.0", "24.22.2"];
    r.recommended_runtime = pins
        .iter()
        .find(|v| r.engines.as_ref().is_none_or(|e| satisfies(v, e)))
        .unwrap_or(&pins[0])
        .to_string();
    if r.engines
        .as_ref()
        .is_some_and(|e| !satisfies(&r.recommended_runtime, e))
    {
        r.warnings.push("No pinned Node LTS runtime satisfies engines.node; the newest pinned LTS is suggested.".into());
    }
    if has("pnpm-lock.yaml") || has("yarn.lock") {
        r.warnings
            .push("Hostify uses npm for now and ignores pnpm and yarn lockfiles.".into());
    }
    if r.site_kind == "static" && has("package.json") {
        r.warnings
            .push("This project has no server start entry and is served as files.".into());
    }
    if r.site_kind == "unknown" {
        r.warnings.push("Looked for a Node start script, main or server entry, Next.js, index.php, composer.json, index.html and dist/index.html.".into());
    }
    r.suggested.install = if has("package-lock.json") {
        Install::Ci
    } else {
        Install::Install
    };
    r.suggested.build_script = r.scripts.contains_key("build").then(|| "build".into());
    r.suggested.start = if scripts["start"].is_string() {
        Start::NpmScript {
            name: "start".into(),
        }
    } else if deps["next"].is_string() {
        Start::Node {
            args: vec![
                "node_modules/next/dist/bin/next".into(),
                "start".into(),
                "-p".into(),
                "{PORT}".into(),
                "-H".into(),
                "127.0.0.1".into(),
            ],
        }
    } else {
        Start::Node {
            args: vec![main.or(entry).unwrap_or("server.js").into()],
        }
    };
    if let Some(sub) = nested_subdir {
        r.suggested.working_dir = sub.clone();
        if r.suggested.document_root == "." || r.suggested.document_root.is_empty() {
            r.suggested.document_root = sub;
        } else {
            r.suggested.document_root = format!("{sub}/{}", r.suggested.document_root);
        }
    }
    Ok(r)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    #[test]
    fn frameworks_folder_and_zip_and_no_env_values() {
        for framework in ["express", "socket.io", "next", "fastify", "koa"] {
            let d = tempfile::tempdir().unwrap();
            let p = format!(
                r#"{{"dependencies":{{"{framework}":"1"}},"scripts":{{"start":"node server.js"}},"engines":{{"node":"^24"}}}}"#
            );
            fs::write(d.path().join("package.json"), &p).unwrap();
            fs::write(
                d.path().join(".env"),
                "PASSWORD=never-report-this\nHELLO=world",
            )
            .unwrap();
            let r = inspect(d.path(), "directory").unwrap();
            assert_eq!(r.framework.as_deref(), Some(framework));
            assert_eq!(r.recommended_runtime, "24.22.2");
            assert!(
                !serde_json::to_string(&r)
                    .unwrap()
                    .contains("never-report-this")
            );
            let z = d.path().join("source.zip");
            let mut w = zip::ZipWriter::new(fs::File::create(&z).unwrap());
            w.start_file(
                "wrapper/package.json",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            w.write_all(p.as_bytes()).unwrap();
            w.finish().unwrap();
            assert_eq!(inspect(&z, "zip").unwrap().site_kind, "node");
        }
    }
    #[test]
    fn classification_errors_limits() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("index.html"), "x").unwrap();
        assert_eq!(inspect(d.path(), "directory").unwrap().site_kind, "static");
        fs::write(d.path().join("package.json"), "bad").unwrap();
        assert!(inspect(d.path(), "directory").is_err());
        fs::write(
            d.path().join("package.json"),
            vec![b' '; LIMIT as usize + 1],
        )
        .unwrap();
        assert!(inspect(d.path(), "directory").is_err());
    }
}
#[cfg(test)]
mod report_regressions {
    use super::*;
    #[test]
    fn classification_metadata_and_runtime_warnings() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("composer.json"), "{}").unwrap();
        assert_eq!(inspect(d.path(), "directory").unwrap().site_kind, "php");
        fs::remove_file(d.path().join("composer.json")).unwrap();
        assert_eq!(inspect(d.path(), "directory").unwrap().site_kind, "unknown");
        fs::create_dir(d.path().join("dist")).unwrap();
        fs::write(d.path().join("dist/index.html"), "files").unwrap();
        fs::write(d.path().join("package.json"),r#"{"name":"passive","engines":{"node":">=99"},"dependencies":{"mysql2":"1"},"scripts":{"prepare":"never execute","build":"build"}}"#).unwrap();
        fs::write(d.path().join("yarn.lock"), "unused").unwrap();
        fs::write(d.path().join(".env"), "PORT=123\nSECRET=never-report").unwrap();
        fs::write(
            d.path().join("package-lock.json"),
            r#"{"packages":{"node_modules/@scope/package":{"hasInstallScript":true}}}"#,
        )
        .unwrap();
        let r = inspect(d.path(), "directory").unwrap();
        assert_eq!(r.site_kind, "static");
        assert_eq!(r.name.as_deref(), Some("passive"));
        assert!(r.uses_database);
        assert!(r.warnings.len() >= 3);
        assert_eq!(r.dotenv_names, ["PORT", "SECRET"]);
        assert_eq!(r.dependencies_with_install_scripts, ["@scope/package"]);
        assert_eq!(r.lifecycle_scripts["prepare"], "never execute");
        assert_eq!(r.lockfile.as_deref(), Some("package-lock"));
        assert!(!serde_json::to_string(&r).unwrap().contains("never-report"));
    }
    #[test]
    fn zip_malformed_oversized_and_unsafe_entries() {
        use std::io::Write;
        let d = tempfile::tempdir().unwrap();
        let z = d.path().join("project.zip");
        for (name, data) in [
            ("package.json", b"bad".to_vec()),
            ("package.json", vec![b' '; LIMIT as usize + 1]),
            ("../package.json", b"{}".to_vec()),
        ] {
            let mut writer = zip::ZipWriter::new(fs::File::create(&z).unwrap());
            writer
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(&data).unwrap();
            writer.finish().unwrap();
            assert!(inspect(&z, "zip").is_err());
        }
    }
}

#[cfg(test)]
mod php_inspection {
    use super::*;
    #[test]
    fn plain_public_wordpress_composer_and_versions() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("index.php"), "<?php").unwrap();
        let r = inspect(d.path(), "directory").unwrap();
        assert_eq!(r.site_kind, "php");
        assert_eq!(r.recommended_runtime, "8.5.12-1");
        fs::remove_file(d.path().join("index.php")).unwrap();
        fs::create_dir(d.path().join("public")).unwrap();
        fs::write(d.path().join("public/index.php"), "<?php").unwrap();
        assert_eq!(
            inspect(d.path(), "directory")
                .unwrap()
                .suggested
                .document_root,
            "public"
        );
        fs::write(d.path().join("wp-config-sample.php"), "<?php").unwrap();
        fs::write(
            d.path().join("package.json"),
            r#"{"scripts":{"start":"asset-builder"}}"#,
        )
        .unwrap();
        let r = inspect(d.path(), "directory").unwrap();
        assert_eq!(r.site_kind, "php");
        assert_eq!(r.framework.as_deref(), Some("wordpress"));
        assert!(r.uses_database);
        assert_eq!(
            r.suggested.persistent_paths,
            ["wp-content", "wp-config.php"]
        );
        fs::remove_file(d.path().join("wp-config-sample.php")).unwrap();
        fs::remove_file(d.path().join("package.json")).unwrap();
        fs::write(
            d.path().join("composer.json"),
            r#"{"require":{"php":">=8.4 <8.5","ext-pdo_mysql":"*","a/b":"1"}}"#,
        )
        .unwrap();
        let r = inspect(d.path(), "directory").unwrap();
        assert_eq!(r.recommended_runtime, "8.4.27-1");
        assert!(r.uses_database);
        assert!(r.warnings.iter().any(|w| w.contains("vendor")));
        fs::create_dir(d.path().join("vendor")).unwrap();
        assert!(
            !inspect(d.path(), "directory")
                .unwrap()
                .warnings
                .iter()
                .any(|w| w.contains("vendor"))
        );
        assert!(php_satisfies("8.5.12-1", "~8.4"));
        assert!(!php_satisfies("8.5.12-1", "~8.4.0"));
        assert!(php_satisfies("8.4.27-1", "^8.4 || ^8.5"));
    }
}
