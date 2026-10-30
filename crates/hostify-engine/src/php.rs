//! Per-site PHP configuration. This config is not an isolation boundary.
use hostify_core::app_config::AppConfig;
use std::path::{Path, PathBuf};

fn quote(path: &Path) -> Result<String, String> {
    let path = path
        .to_str()
        .ok_or("PHP paths must be valid Unicode.")?
        .replace('\\', "/");
    if path.chars().any(|c| c == '"' || c.is_control()) {
        return Err("PHP paths cannot contain quotes or control characters.".into());
    }
    Ok(format!("\"{path}\""))
}
pub(crate) fn generate(
    config: &AppConfig,
    runtime: &Path,
    run: &Path,
    log: &Path,
) -> Result<String, String> {
    config.validate_php()?;
    let mut ini = format!("extension_dir={}\n", quote(&runtime.join("ext"))?);
    for name in &config.php_extensions {
        if !runtime
            .join("ext")
            .join(format!("php_{name}.dll"))
            .is_file()
        {
            return Err(format!(
                "PHP extension {name} is missing from the selected runtime's ext directory."
            ));
        }
        ini.push_str(&format!("extension={name}\n"));
    }
    ini.push_str("cgi.fix_pathinfo=0\ncgi.force_redirect=0\nfastcgi.logging=0\nexpose_php=Off\ndisplay_errors=Off\ndisplay_startup_errors=Off\nlog_errors=On\nallow_url_include=Off\nallow_url_fopen=Off\nsession.cookie_httponly=1\nsession.use_strict_mode=1\nuser_ini.filename=\ndate.timezone=UTC\nvariables_order=GPCS\n");
    for (key, value) in [
        ("error_log", log.to_path_buf()),
        ("session.save_path", run.join("sessions")),
        ("upload_tmp_dir", run.join("tmp")),
        ("sys_temp_dir", run.join("tmp")),
    ] {
        ini.push_str(&format!("{key}={}\n", quote(&value)?));
    }
    for (key, value) in &config.php_settings {
        ini.push_str(&format!("{key}={value}\n"));
    }
    Ok(ini)
}
pub(crate) fn write(
    config: &AppConfig,
    data: &Path,
    id: &str,
    exe: &Path,
) -> Result<PathBuf, String> {
    let run = data.join("run/php").join(id);
    let log = data.join("logs").join(format!("{id}.php-error.log"));
    let ini = generate(
        config,
        exe.parent().ok_or("Invalid PHP runtime path.")?,
        &run,
        &log,
    )?;
    for dir in [run.join("sessions"), run.join("tmp"), data.join("logs")] {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let path = run.join("php.ini");
    let temp = run.join(format!("{}.tmp", uuid::Uuid::new_v4()));
    std::fs::write(&temp, ini).map_err(|e| e.to_string())?;
    if let Err(e) = std::fs::rename(&temp, &path) {
        let _ = std::fs::remove_file(temp);
        return Err(e.to_string());
    }
    Ok(path)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validated_ini_and_windows_paths() {
        let mut c = AppConfig {
            php_extensions: vec![],
            ..Default::default()
        };
        let ini = generate(
            &c,
            Path::new(r"C:\PHP"),
            Path::new(r"D:\run\site"),
            Path::new(r"D:\logs\site.log"),
        )
        .unwrap();
        assert!(ini.contains("extension_dir=\"C:/PHP/ext\""));
        assert!(ini.contains("user_ini.filename=\n"));
        assert!(!ini.contains("open_basedir"));
        c.php_settings
            .insert("memory_limit".into(), "128M\nextension=evil".into());
        assert!(generate(&c, Path::new("runtime"), Path::new("run"), Path::new("log")).is_err());
        c.php_settings.clear();
        c.php_extensions.push("evil".into());
        assert!(
            generate(&c, Path::new("runtime"), Path::new("run"), Path::new("log"))
                .unwrap_err()
                .contains("not allowed")
        );
        assert!(quote(Path::new("bad\"path")).is_err());
        assert!(quote(Path::new("bad\npath")).is_err());
    }
}
