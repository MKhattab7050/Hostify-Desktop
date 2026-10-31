//! Managed MySQL through the pinned publisher clients. SQL is supplied on stdin.
use crate::state::Store;
use anyhow::{Result, anyhow, bail};
use hostify_core::secrets::{SecretStore, WindowsCredentialStore};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
    time::Instant,
};
pub trait Sealer: Send + Sync {
    fn seal(&self, v: &[u8]) -> Result<Vec<u8>>;
    fn open(&self, v: &[u8]) -> Result<Vec<u8>>;
}
pub struct Dpapi;
impl Sealer for Dpapi {
    fn seal(&self, v: &[u8]) -> Result<Vec<u8>> {
        hostify_core::vault::seal(v).map_err(|e| anyhow!(e))
    }
    fn open(&self, v: &[u8]) -> Result<Vec<u8>> {
        hostify_core::vault::open(v).map_err(|e| anyhow!(e))
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Config {
    pub version: String,
    pub port: u16,
    pub enabled: bool,
}
pub struct Manager {
    pub store: Arc<Store>,
    secrets: Arc<dyn SecretStore>,
    sealer: Arc<dyn Sealer>,
    pub operation: tokio::sync::Mutex<()>,
}
pub fn validate_database(name: &str) -> Result<()> {
    validate_name(name, 48)?;
    if ["mysql", "sys", "information_schema", "performance_schema"].contains(&name) {
        bail!("That database name is reserved by MySQL.")
    }
    Ok(())
}
pub fn validate_user(name: &str) -> Result<()> {
    validate_name(name, 32)
}
fn validate_name(name: &str, limit: usize) -> Result<()> {
    if name.is_empty()
        || name.len() > limit
        || !name.as_bytes()[0].is_ascii_lowercase()
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    {
        bail!(
            "Names must start with a lowercase letter and contain only lowercase ASCII letters, digits and underscores (at most {limit} characters)."
        )
    }
    Ok(())
}
fn quote(path: &Path) -> Result<String> {
    let v = path
        .to_str()
        .ok_or_else(|| anyhow!("MySQL paths must be valid Unicode."))?
        .replace('\\', "/");
    if v.chars().any(|c| c == '"' || c.is_control()) {
        bail!("MySQL paths cannot contain quotes or control characters.")
    }
    Ok(format!("\"{v}\""))
}
pub fn generate_ini(runtime: &Path, root: &Path, port: u16) -> Result<String> {
    if port == 0 {
        bail!("MySQL needs a nonzero port.")
    }
    Ok(format!(
        "[mysqld]\nbasedir={}\ndatadir={}\nport={port}\nbind-address=127.0.0.1\nmysqlx=OFF\nlocal_infile=OFF\nskip-name-resolve=ON\nlog-error={}\nenable-named-pipe=OFF\nshared-memory=OFF\ninnodb_buffer_pool_size=128M\nperformance_schema=OFF\nmax_connections=100\ncharacter-set-server=utf8mb4\nsecure-file-priv=NULL\n",
        quote(runtime)?,
        quote(&root.join("mysql/data"))?,
        quote(&root.join("logs/mysql-error.log"))?
    ))
}
pub fn password() -> Result<String> {
    #[cfg(windows)]
    {
        #[link(name = "bcrypt")]
        unsafe extern "system" {
            fn BCryptGenRandom(
                h: *mut std::ffi::c_void,
                bytes: *mut u8,
                len: u32,
                flags: u32,
            ) -> i32;
        }
        const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
        let mut out = String::new();
        while out.len() < 32 {
            let mut b = [0u8; 64];
            if unsafe { BCryptGenRandom(std::ptr::null_mut(), b.as_mut_ptr(), 64, 2) } < 0 {
                bail!("Windows could not generate a MySQL password.")
            }
            for x in b {
                if x < 248 && out.len() < 32 {
                    out.push(A[(x % 62) as usize] as char)
                }
            }
        }
        Ok(out)
    }
    #[cfg(not(windows))]
    {
        bail!("MySQL management requires Windows.")
    }
}
pub fn restrict(path: &Path, system: bool) -> Result<()> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::{
            Foundation::LocalFree,
            Security::{Authorization::*, *},
        };
        let sid = crate::windows::sid()?;
        let flags = if path.is_dir() { "OICI" } else { "" };
        let text = crate::windows::wide(&format!(
            "D:P(A;{flags};FA;;;{sid}){}",
            if system {
                format!("(A;{flags};FA;;;SY)")
            } else {
                String::new()
            }
        ));
        let mut sd = std::ptr::null_mut();
        unsafe {
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                text.as_ptr(),
                1,
                &mut sd,
                std::ptr::null_mut(),
            ) == 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            let result = (|| {
                let mut present = 0;
                let mut defaulted = 0;
                let mut acl = std::ptr::null_mut();
                if GetSecurityDescriptorDacl(sd, &mut present, &mut acl, &mut defaulted) == 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
                let wide = crate::windows::wide(
                    path.to_str()
                        .ok_or_else(|| anyhow!("Invalid MySQL path."))?,
                );
                let code = SetNamedSecurityInfoW(
                    wide.as_ptr(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    acl,
                    std::ptr::null(),
                );
                if code != 0 {
                    return Err(std::io::Error::from_raw_os_error(code as i32).into());
                }
                Ok(())
            })();
            LocalFree(sd);
            result
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (path, system);
        bail!("MySQL management requires Windows.")
    }
}
struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn atomic(path: &Path, text: &str) -> Result<()> {
    let temp = Temp(path.with_file_name(format!("{}.tmp", uuid::Uuid::new_v4())));
    fs::write(&temp.0, text)?;
    fs::rename(&temp.0, path)?;
    Ok(())
}
fn restricted_temp(root: &Path, text: &str) -> Result<Temp> {
    let run = root.join("run");
    fs::create_dir_all(&run)?;
    let temp = Temp(run.join(format!("mysql-{}.cnf", uuid::Uuid::new_v4())));
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp.0)?;
    restrict(&temp.0, false)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    Ok(temp)
}
fn command(exe: &Path) -> Command {
    let mut cmd = Command::new(exe);
    cmd.env_clear();
    for k in ["SystemRoot", "WINDIR", "TEMP", "TMP"] {
        if let Some(v) = std::env::var_os(k) {
            cmd.env(k, v);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    cmd
}
impl Manager {
    pub fn production(store: Arc<Store>) -> Arc<Self> {
        Self::new(store, Arc::new(WindowsCredentialStore), Arc::new(Dpapi))
    }
    pub fn new(
        store: Arc<Store>,
        secrets: Arc<dyn SecretStore>,
        sealer: Arc<dyn Sealer>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            secrets,
            sealer,
            operation: tokio::sync::Mutex::new(()),
        })
    }
    pub fn config(&self) -> Result<Option<Config>> {
        match fs::read(self.store.root.join("mysql/.hostify-initialized")) {
            Ok(b) => Ok(Some(serde_json::from_slice(&b)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    pub fn required_config(&self) -> Result<Config> {
        self.config()?.ok_or_else(|| anyhow!("Set up MySQL first."))
    }
    pub fn root_password(&self) -> Result<String> {
        self.secrets
            .get("mysql-root")
            .map_err(|_| anyhow!("Could not read the MySQL root credential."))?
            .ok_or_else(|| anyhow!("The MySQL root credential is missing."))
    }
    pub fn executable(&self, c: &Config, tool: &str) -> Result<PathBuf> {
        let server = self
            .store
            .runtimes
            .resolve("mysql", &c.version)
            .map_err(|e| anyhow!(e))?;
        Ok(server
            .parent()
            .ok_or_else(|| anyhow!("Invalid MySQL runtime."))?
            .join(format!("{tool}.exe")))
    }
    pub fn progress(&self, op: &str, database: Option<&str>, state: &str, detail: &str) {
        self.store.emit(
            "mysql-progress",
            json!({"operation":op,"database":database,"state":state,"detail":detail}),
        );
    }
    pub fn redact(&self, text: &str) -> String {
        let mut text = text.to_owned();
        if let Ok(root) = self.root_password() {
            text = text.replace(&root, "[REDACTED]");
        }
        if let Ok(db) = self.store.db()
            && let Ok(mut st) = db.prepare("SELECT sealed_password FROM managed_db_users")
            && let Ok(rows) = st.query_map([], |r| r.get::<_, Vec<u8>>(0))
        {
            for v in rows.flatten() {
                if let Ok(b) = self.sealer.open(&v)
                    && let Ok(s) = String::from_utf8(b)
                    && !s.is_empty()
                {
                    text = text.replace(&s, "[REDACTED]");
                }
            }
        }
        text
    }
    pub fn error_log(&self) -> String {
        self.redact(
            &crate::supervisor::tail_logs(&self.store.root.join("logs/mysql-error.log"), 20)
                .unwrap_or_default()
                .join("\n"),
        )
    }
    pub fn setup(&self, version: &str) -> Result<()> {
        if self.config()?.is_some() {
            bail!("MySQL has already been set up.")
        }
        let server = self
            .store
            .runtimes
            .resolve("mysql", version)
            .map_err(|e| anyhow!(e))?;
        let password = password()?;
        self.secrets
            .set("mysql-root", &password)
            .map_err(|_| anyhow!("Could not store the MySQL root credential."))?;
        let root = &self.store.root;
        for p in [
            root.join("mysql"),
            root.join("mysql/data"),
            root.join("backups/mysql"),
        ] {
            fs::create_dir_all(&p)?;
            restrict(&p, true)?;
        }
        let data = root.join("mysql/data");
        let entries = fs::read_dir(&data)?.collect::<std::io::Result<Vec<_>>>()?;
        for e in &entries {
            let n = e.file_name().to_string_lossy().into_owned();
            if (e.file_type()?.is_dir()
                && ![
                    "mysql",
                    "sys",
                    "performance_schema",
                    "#innodb_redo",
                    "#innodb_temp",
                ]
                .contains(&n.as_str()))
                || (n.ends_with(".ibd") && n != "mysql.ibd")
            {
                bail!(
                    "The incomplete MySQL data directory contains user databases. It was not cleared: {}.",
                    data.display()
                )
            }
        }
        if !entries.is_empty() {
            if !fs::canonicalize(&data)?.starts_with(fs::canonicalize(root.join("mysql"))?) {
                bail!("MySQL data points outside its managed directory.")
            }
            fs::remove_dir_all(&data)?;
            fs::create_dir(&data)?;
            restrict(&data, true)?;
        }
        let port = (3307..=65535)
            .find(|p| crate::port_available(*p))
            .ok_or_else(|| anyhow!("No free MySQL port is available."))?;
        let base = server
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| anyhow!("Invalid MySQL runtime."))?;
        let ini = root.join("mysql/my.ini");
        atomic(&ini, &generate_ini(base, root, port)?)?;
        let init = restricted_temp(
            root,
            &format!(
                "ALTER USER 'root'@'localhost' IDENTIFIED BY '{password}';\nCREATE USER IF NOT EXISTS 'root'@'127.0.0.1' IDENTIFIED BY '{password}';\nGRANT ALL PRIVILEGES ON *.* TO 'root'@'127.0.0.1' WITH GRANT OPTION;\n"
            ),
        )?;
        let output = command(&server)
            .arg(format!("--defaults-file={}", ini.display()))
            .arg("--initialize")
            .arg("--log-error-verbosity=1")
            .arg(format!("--init-file={}", init.0.display()))
            .output()?;
        if !output.status.success() {
            bail!(
                "MySQL initialization failed ({}): {} {}",
                output.status,
                self.redact(&String::from_utf8_lossy(&output.stderr)),
                self.error_log()
            )
        }
        if fs::read_to_string(root.join("logs/mysql-error.log"))
            .unwrap_or_default()
            .contains(&password)
        {
            bail!("MySQL wrote a credential to its error log. Setup was not completed.")
        }
        atomic(
            &root.join("mysql/.hostify-initialized"),
            &serde_json::to_string(&Config {
                version: version.into(),
                port,
                enabled: true,
            })?,
        )?;
        Ok(())
    }
    fn client(
        &self,
        tool: &str,
        user: &str,
        password: &str,
        database: Option<&str>,
    ) -> Result<(Command, Temp)> {
        if user != "root" {
            validate_user(user)?
        }
        if let Some(db) = database {
            validate_database(db)?
        }
        if password.len() != 32 || !password.bytes().all(|b| b.is_ascii_alphanumeric()) {
            bail!("The stored MySQL credential is invalid.")
        }
        let c = self.required_config()?;
        let options = restricted_temp(
            &self.store.root,
            &format!(
                "[client]\nuser={user}\npassword={password}\nhost=127.0.0.1\nport={}\nprotocol=TCP\n",
                c.port
            ),
        )?;
        let mut cmd = command(&self.executable(&c, tool)?);
        cmd.arg(format!("--defaults-extra-file={}", options.0.display()));
        if let Some(db) = database {
            cmd.arg(format!("--database={db}"));
        }
        Ok((cmd, options))
    }
    pub fn run_sql(
        &self,
        user: &str,
        password: &str,
        database: Option<&str>,
        sql: &str,
    ) -> Result<String> {
        let (mut cmd, _guard) = self.client("mysql", user, password, database)?;
        cmd.args([
            "--batch",
            "--skip-column-names",
            "--raw",
            "--connect-timeout=5",
        ]);
        self.run_input(cmd, sql.as_bytes(), password)
    }
    fn run_input(&self, mut cmd: Command, input: impl Read, password: &str) -> Result<String> {
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let out = std::thread::spawn(move || {
            let mut b = Vec::new();
            let mut r = stdout;
            r.read_to_end(&mut b).map(|_| b)
        });
        let err = std::thread::spawn(move || {
            let mut b = Vec::new();
            let mut r = stderr;
            r.read_to_end(&mut b).map(|_| b)
        });
        let write = {
            let mut stdin = child.stdin.take().unwrap();
            std::io::copy(&mut std::io::BufReader::new(input), &mut stdin)
        };
        let status = child.wait()?;
        let output = out
            .join()
            .map_err(|_| anyhow!("MySQL output reader failed."))??;
        let error = err
            .join()
            .map_err(|_| anyhow!("MySQL error reader failed."))??;
        if !status.success() {
            bail!(
                "MySQL client failed ({status}): {}",
                self.redact(&String::from_utf8_lossy(&error).replace(password, "[REDACTED]"))
            )
        }
        write?;
        Ok(self.redact(&String::from_utf8_lossy(&output).replace(password, "[REDACTED]")))
    }
    pub fn sql(&self, sql: &str) -> Result<String> {
        self.run_sql("root", &self.root_password()?, None, sql)
    }
    pub fn shutdown_server(&self) -> Result<()> {
        let password = self.root_password()?;
        let (mut cmd, _guard) = self.client("mysqladmin", "root", &password, None)?;
        cmd.args(["--connect-timeout=5", "--shutdown-timeout=60", "shutdown"]);
        self.run_input(cmd, std::io::empty(), &password)?;
        Ok(())
    }
    pub fn managed(&self, name: &str) -> Result<Option<String>> {
        validate_database(name)?;
        self.store
            .db()?
            .query_row(
                "SELECT site_id FROM managed_databases WHERE name=?1",
                [name],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| {
                anyhow!("Hostify did not create this database; it cannot manage or drop it.")
            })
    }
    pub fn user(&self, user: &str) -> Result<(String, String)> {
        validate_user(user)?;
        let (name, sealed): (String, Vec<u8>) = self
            .store
            .db()?
            .query_row(
                "SELECT database_name,sealed_password FROM managed_db_users WHERE user_name=?1",
                [user],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .ok_or_else(|| anyhow!("Managed MySQL user not found."))?;
        let password = String::from_utf8(self.sealer.open(&sealed)?)
            .map_err(|_| anyhow!("Could not decode the MySQL credential."))?;
        Ok((name, password))
    }
    fn users(&self, name: &str) -> Result<Vec<String>> {
        let db = self.store.db()?;
        let mut st=db.prepare("SELECT user_name FROM managed_db_users WHERE database_name=?1 ORDER BY created_at,user_name")?;
        Ok(st
            .query_map([name], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }
    fn credentials(&self, name: &str) -> Result<(String, String)> {
        self.managed(name)?;
        let user = self
            .users(name)?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("This database has no managed user."))?;
        let (_, password) = self.user(&user)?;
        Ok((user, password))
    }
    fn env_values(&self, name: &str, user: &str, password: &str) -> Result<Vec<(String, String)>> {
        Ok(vec![
            ("DB_HOST".into(), "127.0.0.1".into()),
            ("DB_PORT".into(), self.required_config()?.port.to_string()),
            ("DB_NAME".into(), name.into()),
            ("DB_DATABASE".into(), name.into()),
            ("DB_USER".into(), user.into()),
            ("DB_USERNAME".into(), user.into()),
            ("DB_PASSWORD".into(), password.into()),
        ])
    }
    pub fn ensure_site_env(&self, site: &str, name: &str) -> Result<()> {
        let (user, password) = self.credentials(name)?;
        let values = self.env_values(name, &user, &password)?;
        self.inject(site, &values, true)
    }
    fn env_read(
        &self,
        db: &rusqlite::Connection,
        site: &str,
        name: &str,
    ) -> Result<Option<String>> {
        let row: Option<(Option<String>, Option<Vec<u8>>)> = db
            .query_row(
                "SELECT plain_value,sealed_value FROM env_vars WHERE site_id=?1 AND name=?2",
                params![site, name],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        row.map(|(plain, sealed)| {
            if let Some(v) = plain {
                Ok(v)
            } else {
                String::from_utf8(self.sealer.open(&sealed.unwrap_or_default())?)
                    .map_err(|_| anyhow!("Could not decode variable {name}."))
            }
        })
        .transpose()
    }
    fn check_env(
        &self,
        db: &rusqlite::Connection,
        site: &str,
        values: &[(String, String)],
        replace: bool,
    ) -> Result<()> {
        db.query_row("SELECT id FROM websites WHERE id=?1", [site], |_| Ok(()))
            .optional()?
            .ok_or_else(|| anyhow!("Website not found."))?;
        let mut conflicts = Vec::new();
        for (name, value) in values {
            if let Some(old) = self.env_read(db, site, name)?
                && old != *value
            {
                conflicts.push(name.clone());
            }
        }
        if !replace && !conflicts.is_empty() {
            bail!(
                "Database environment variables conflict: {}. Use replaceEnv to replace them.",
                conflicts.join(", ")
            )
        }
        Ok(())
    }
    fn inject(&self, site: &str, values: &[(String, String)], replace: bool) -> Result<()> {
        let mut db = self.store.db()?;
        let tx = db.transaction()?;
        self.check_env(&tx, site, values, replace)?;
        for (name, value) in values {
            let secret = name == "DB_PASSWORD";
            let sealed = if secret {
                Some(self.sealer.seal(value.as_bytes())?)
            } else {
                None
            };
            let plain = if secret { None } else { Some(value) };
            tx.execute("INSERT INTO env_vars(site_id,name,secret,plain_value,sealed_value,updated_at) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(site_id,name) DO UPDATE SET secret=excluded.secret,plain_value=excluded.plain_value,sealed_value=excluded.sealed_value,updated_at=excluded.updated_at",params![site,name,secret,plain,sealed,chrono::Utc::now().to_rfc3339()])?;
        }
        let name = &values.iter().find(|(k, _)| k == "DB_NAME").unwrap().1;
        let user = &values.iter().find(|(k, _)| k == "DB_USER").unwrap().1;
        tx.execute("INSERT INTO application_settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![format!("mysql-env:{name}"),serde_json::to_string(&json!({"site":site,"user":user}))?])?;
        tx.commit()?;
        Ok(())
    }
    fn remove_injected(&self, name: &str) -> Result<()> {
        let key = format!("mysql-env:{name}");
        if let Some(snapshot) = self.store.saved(&key)?
            && let (Some(site), Some(user)) = (snapshot["site"].as_str(), snapshot["user"].as_str())
        {
            let (_, password) = self.user(user)?;
            let values = self.env_values(name, user, &password)?;
            let mut db = self.store.db()?;
            let tx = db.transaction()?;
            for (k, value) in values {
                if self.env_read(&tx, site, &k)?.as_deref() == Some(&value) {
                    tx.execute(
                        "DELETE FROM env_vars WHERE site_id=?1 AND name=?2",
                        params![site, k],
                    )?;
                }
            }
            tx.execute("DELETE FROM application_settings WHERE key=?1", [key])?;
            tx.commit()?;
        }
        Ok(())
    }
    fn restart_required(&self, site: Option<&str>) -> bool {
        site.and_then(|id| self.store.site(id).ok())
            .is_some_and(|s| {
                s.kind != "static" && matches!(s.status.as_str(), "starting" | "running" | "error")
            })
    }
    fn grant(&self, name: &str, user: &str, password: &str) -> Result<()> {
        validate_database(name)?;
        validate_user(user)?;
        self.sql(&format!(
            "CREATE USER '{user}'@'127.0.0.1' IDENTIFIED BY '{password}';"
        ))
        .map_err(|e| anyhow!(e.to_string().replace(password, "[REDACTED]")))?;
        if let Err(e)=self.sql(&format!("GRANT SELECT, INSERT, UPDATE, DELETE, CREATE, ALTER, INDEX, DROP, REFERENCES, CREATE TEMPORARY TABLES, LOCK TABLES, EXECUTE, CREATE VIEW, SHOW VIEW, CREATE ROUTINE, ALTER ROUTINE, TRIGGER ON `{name}`.* TO '{user}'@'127.0.0.1';")){let cleanup=self.sql(&format!("DROP USER '{user}'@'127.0.0.1';"));bail!("{} User cleanup: {}.",e.to_string().replace(password,"[REDACTED]"),if cleanup.is_ok(){"succeeded"}else{"failed"})}
        Ok(())
    }
    fn add_user(&self, name: &str, user: &str) -> Result<()> {
        self.managed(name)?;
        validate_user(user)?;
        let password = password()?;
        let sealed = self.sealer.seal(password.as_bytes())?;
        self.grant(name, user, &password)?;
        let result = self.store.db()?.execute(
            "INSERT INTO managed_db_users VALUES(?1,?2,?3,?4)",
            params![user, name, sealed, chrono::Utc::now().to_rfc3339()],
        );
        if let Err(e) = result {
            let cleanup = self.sql(&format!("DROP USER '{user}'@'127.0.0.1';"));
            bail!(
                "Could not record the database user: {e}. Cleanup {}.",
                if cleanup.is_ok() {
                    "succeeded"
                } else {
                    "failed"
                }
            )
        }
        Ok(())
    }
    pub fn create_database(
        &self,
        name: &str,
        site: Option<&str>,
        inject: bool,
        replace: bool,
    ) -> Result<Value> {
        validate_database(name)?;
        let user = format!("{name}_app");
        validate_user(&user)?;
        if inject && site.is_none() {
            bail!("Choose a website before injecting database variables.")
        }
        let password = password()?;
        let sealed = self.sealer.seal(password.as_bytes())?;
        let values = self.env_values(name, &user, &password)?;
        if let Some(site) = site {
            self.store.site(site)?;
            if inject {
                self.check_env(&*self.store.db()?, site, &values, replace)?;
            }
        }
        if self
            .store
            .db()?
            .query_row(
                "SELECT 1 FROM managed_databases WHERE name=?1",
                [name],
                |_| Ok(()),
            )
            .optional()?
            .is_some()
        {
            bail!("This database is already managed by Hostify.")
        }
        self.sql(&format!("CREATE DATABASE `{name}` CHARACTER SET utf8mb4;"))?;
        if let Err(e) = self.grant(name, &user, &password) {
            let cleanup = self.sql(&format!("DROP DATABASE `{name}`;"));
            bail!(
                "{} Database cleanup {}.",
                e,
                if cleanup.is_ok() {
                    "succeeded"
                } else {
                    "failed"
                }
            )
        }
        let result = (|| -> Result<()> {
            {
                let mut db = self.store.db()?;
                let tx = db.transaction()?;
                tx.execute(
                    "INSERT INTO managed_databases(name,site_id,created_at) VALUES(?1,?2,?3)",
                    params![name, site, chrono::Utc::now().to_rfc3339()],
                )?;
                tx.execute(
                    "INSERT INTO managed_db_users VALUES(?1,?2,?3,?4)",
                    params![user, name, sealed, chrono::Utc::now().to_rfc3339()],
                )?;
                tx.commit()?;
            }
            if inject {
                self.inject(site.unwrap(), &values, replace)?;
            }
            Ok(())
        })();
        if let Err(e) = result {
            let du = self.sql(&format!("DROP USER '{user}'@'127.0.0.1';"));
            let dd = self.sql(&format!("DROP DATABASE `{name}`;"));
            if du.is_ok() && dd.is_ok() {
                self.store
                    .db()?
                    .execute("DELETE FROM managed_databases WHERE name=?1", [name])?;
            }
            bail!(
                "{} Cleanup: user {}, database {}.",
                e,
                if du.is_ok() { "removed" } else { "failed" },
                if dd.is_ok() { "removed" } else { "failed" }
            )
        }
        Ok(json!({"restartRequired":inject&&self.restart_required(site)}))
    }
    pub fn delete_database(&self, name: &str, confirmation: &str) -> Result<()> {
        self.managed(name)?;
        if name != confirmation {
            bail!("Type the database name exactly to confirm deletion.")
        }
        self.remove_injected(name)?;
        for user in self.users(name)? {
            validate_user(&user)?;
            self.sql(&format!("DROP USER IF EXISTS '{user}'@'127.0.0.1';"))?;
        }
        self.sql(&format!("DROP DATABASE `{name}`;"))?;
        self.store
            .db()?
            .execute("DELETE FROM managed_databases WHERE name=?1", [name])?;
        Ok(())
    }
    pub fn connection(&self, name: &str) -> Result<Value> {
        let (user, _) = self.credentials(name)?;
        Ok(
            json!({"host":"127.0.0.1","port":self.required_config()?.port,"database":name,"user":user}),
        )
    }
    pub fn test_connection(&self, name: &str) -> Result<Value> {
        let (user, password) = self.credentials(name)?;
        let now = Instant::now();
        let result = self.run_sql(&user, &password, Some(name), "SELECT 1;");
        Ok(
            json!({"ok":result.is_ok(),"detail":match result{Ok(_)=>"Connected successfully.".into(),Err(e)=>self.redact(&e.to_string())},"elapsedMs":now.elapsed().as_millis()as u64}),
        )
    }
    fn size(&self, name: &str) -> Result<u64> {
        validate_database(name)?;
        Ok(self.sql(&format!("SELECT COALESCE(SUM(DATA_LENGTH+INDEX_LENGTH),0) FROM information_schema.TABLES WHERE TABLE_SCHEMA='{name}';"))?.trim().parse()?)
    }
    pub fn list(&self, running: bool) -> Result<Value> {
        let names: Vec<(String, Option<String>, Option<String>, Option<u64>)> = {
            let db = self.store.db()?;
            let mut st=db.prepare("SELECT name,site_id,last_backup_at,last_backup_bytes FROM managed_databases ORDER BY name")?;
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut rows = Vec::new();
        for (name, site, at, bytes) in names {
            let mut v = json!({"name":name,"siteId":site,"siteName":site.as_deref().and_then(|id|self.store.site(id).ok()).map(|s|s.name),"users":self.users(&name)?,"connectionOk":if running{Some(self.test_connection(&name)?["ok"].as_bool().unwrap_or(false))}else{None},"lastBackupAt":at,"lastBackupBytes":bytes});
            if running && let Ok(size) = self.size(&name) {
                v["sizeBytes"] = json!(size);
            }
            rows.push(v);
        }
        Ok(json!(rows))
    }
    pub fn backup_dir(&self, name: &str) -> Result<PathBuf> {
        self.managed(name)?;
        let base = self.store.root.join("backups/mysql");
        fs::create_dir_all(&base)?;
        restrict(&base, true)?;
        let path = base.join(name);
        fs::create_dir_all(&path)?;
        if !fs::canonicalize(&path)?.starts_with(fs::canonicalize(&base)?) {
            bail!("The backup folder points outside managed MySQL backups.")
        }
        Ok(path)
    }
    pub fn backup_file(&self, name: &str, file: &str) -> Result<PathBuf> {
        validate_backup_filename(file)?;
        let dir = self.backup_dir(name)?;
        let path = dir.join(file);
        if fs::canonicalize(&path)?.parent() != Some(fs::canonicalize(&dir)?.as_path()) {
            bail!("The backup file points outside this database's backup folder.")
        }
        if !fs::symlink_metadata(&path)?.is_file() {
            bail!("Choose a regular backup file.")
        }
        Ok(path)
    }
    pub fn backup(&self, name: &str) -> Result<Value> {
        self.managed(name)?;
        let dir = self.backup_dir(name)?;
        let size = self.size(name)?;
        if free_disk(&dir)? < size.saturating_mul(2).saturating_add(16 * 1024 * 1024) {
            bail!("There is not enough free disk space for this database backup.")
        }
        let file = format!(
            "{}-{}.sql",
            chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ"),
            uuid::Uuid::new_v4()
        );
        let partial = Temp(dir.join(format!("{file}.partial")));
        let output = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&partial.0)?;
        let password = self.root_password()?;
        let (mut cmd, _guard) = self.client("mysqldump", "root", &password, None)?;
        let out = cmd
            .args([
                "--single-transaction",
                "--routines",
                "--triggers",
                "--events",
                "--no-tablespaces",
                name,
            ])
            .stdout(Stdio::from(output))
            .stderr(Stdio::piped())
            .output()?;
        if !out.status.success() {
            bail!(
                "mysqldump failed ({}): {}",
                out.status,
                self.redact(&String::from_utf8_lossy(&out.stderr))
            )
        }
        validate_dump(&partial.0)?;
        fs::rename(&partial.0, dir.join(&file))?;
        let bytes = fs::metadata(dir.join(&file))?.len();
        let time = chrono::Utc::now().to_rfc3339();
        self.store.db()?.execute("UPDATE managed_databases SET last_backup_at=?1,last_backup_path=?2,last_backup_bytes=?3 WHERE name=?4",params![time,file,bytes,name])?;
        Ok(json!({"file":file,"sizeBytes":bytes,"time":time,"createdAt":time}))
    }
    pub fn backups(&self, name: &str) -> Result<Value> {
        let mut files = Vec::new();
        for entry in fs::read_dir(self.backup_dir(name)?)? {
            let entry = entry?;
            let file = entry.file_name().to_string_lossy().into_owned();
            if validate_backup_filename(&file).is_ok() && self.backup_file(name, &file).is_ok() {
                let meta = entry.metadata()?;
                let time = meta
                    .modified()
                    .ok()
                    .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339());
                files
                    .push(json!({"file":file,"sizeBytes":meta.len(),"time":time,"createdAt":time}));
            }
        }
        files.sort_by(|a, b| b["file"].as_str().cmp(&a["file"].as_str()));
        Ok(json!(files))
    }
    pub fn restore_file(&self, name: &str, file: &str) -> Result<()> {
        let path = self.backup_file(name, file)?;
        validate_dump(&path)?;
        let password = self.root_password()?;
        let (cmd, _guard) = self.client("mysql", "root", &password, Some(name))?;
        self.run_input(cmd, fs::File::open(path)?, &password)?;
        Ok(())
    }
    pub fn linked_sites(&self, name: &str) -> Result<Vec<String>> {
        Ok(self.managed(name)?.into_iter().collect())
    }
    pub fn should_start(&self) -> Result<bool> {
        Ok(self.config()?.is_some_and(|c| c.enabled)
            || self
                .store
                .db()?
                .query_row("SELECT count(*) FROM managed_databases", [], |r| {
                    r.get::<_, u64>(0)
                })?
                > 0)
    }
    pub fn linked_database(&self, site: &str) -> Result<Option<String>> {
        Ok(self
            .store
            .db()?
            .query_row(
                "SELECT name FROM managed_databases WHERE site_id=?1 ORDER BY name LIMIT 1",
                [site],
                |r| r.get(0),
            )
            .optional()?)
    }
    pub fn request(&self, method: &str, p: &Value, running: bool) -> Result<Value> {
        let arg = |key: &str| -> Result<&str> {
            p[key]
                .as_str()
                .ok_or_else(|| anyhow!("Missing argument {key}."))
        };
        let flag = |key: &str| p[key].as_bool().unwrap_or(false);
        match method {
            "mysql_setup" => {
                self.setup(arg("version")?)?;
                Ok(Value::Null)
            }
            "list_databases" => self.list(running),
            "create_database" => self.create_database(
                arg("name")?,
                p["siteId"].as_str(),
                flag("injectEnv"),
                flag("replaceEnv"),
            ),
            "delete_database" => {
                self.delete_database(arg("name")?, arg("confirmName")?)?;
                Ok(Value::Null)
            }
            "create_db_user" => {
                self.add_user(arg("database")?, arg("user")?)?;
                Ok(Value::Null)
            }
            "reveal_db_password" => Ok(json!(self.user(arg("user")?)?.1)),
            "delete_db_user" => {
                let user = arg("user")?;
                let (name, _) = self.user(user)?;
                let snapshot = self.store.saved(&format!("mysql-env:{name}"))?;
                if snapshot.as_ref().is_some_and(|s| s["user"] == user) {
                    self.remove_injected(&name)?;
                }
                self.sql(&format!("DROP USER '{user}'@'127.0.0.1';"))?;
                self.store
                    .db()?
                    .execute("DELETE FROM managed_db_users WHERE user_name=?1", [user])?;
                Ok(Value::Null)
            }
            "rotate_db_password" => {
                let user = arg("user")?;
                let (name, old) = self.user(user)?;
                let password = password()?;
                let sealed = self.sealer.seal(password.as_bytes())?;
                self.sql(&format!(
                    "ALTER USER '{user}'@'127.0.0.1' IDENTIFIED BY '{password}';"
                ))
                .map_err(|e| anyhow!(e.to_string().replace(&password, "[REDACTED]")))?;
                let result = self.store.db()?.execute(
                    "UPDATE managed_db_users SET sealed_password=?1 WHERE user_name=?2",
                    params![sealed, user],
                );
                if let Err(e) = result {
                    let rollback = self.sql(&format!(
                        "ALTER USER '{user}'@'127.0.0.1' IDENTIFIED BY '{old}';"
                    ));
                    bail!(
                        "Could not record the rotated password: {e}. Rollback {}.",
                        if rollback.is_ok() {
                            "succeeded"
                        } else {
                            "failed"
                        }
                    )
                }
                let snapshot = self.store.saved(&format!("mysql-env:{name}"))?;
                let mut restart = false;
                if let Some(snapshot) = snapshot
                    && snapshot["user"] == user
                    && let Some(site) = snapshot["site"].as_str()
                {
                    let db = self.store.db()?;
                    let current = self.env_read(&db, site, "DB_PASSWORD")?;
                    drop(db);
                    if current.as_deref() == Some(&old) {
                        self.inject(site, &self.env_values(&name, user, &password)?, true)?;
                        restart = self.restart_required(Some(site));
                    }
                }
                Ok(json!({"restartRequired":restart}))
            }
            "database_connection" => self.connection(arg("name")?),
            "test_connection" => self.test_connection(arg("name")?),
            "backup_database" => self.backup(arg("name")?),
            "list_backups" => self.backups(arg("name")?),
            "delete_backup" => {
                fs::remove_file(self.backup_file(arg("name")?, arg("file")?)?)?;
                Ok(Value::Null)
            }
            "link_database" => {
                let name = arg("name")?;
                self.managed(name)?;
                let site = arg("siteId")?;
                self.store.site(site)?;
                let (user, password) = self.credentials(name)?;
                let values = self.env_values(name, &user, &password)?;
                if flag("injectEnv") {
                    self.check_env(&*self.store.db()?, site, &values, flag("replaceEnv"))?;
                }
                self.remove_injected(name)?;
                if flag("injectEnv") {
                    self.inject(site, &values, flag("replaceEnv"))?;
                }
                self.store.db()?.execute(
                    "UPDATE managed_databases SET site_id=?1 WHERE name=?2",
                    params![site, name],
                )?;
                Ok(json!({"restartRequired":flag("injectEnv")&&self.restart_required(Some(site))}))
            }
            "unlink_database" => {
                let name = arg("name")?;
                let site = self.managed(name)?;
                self.remove_injected(name)?;
                self.store.db()?.execute(
                    "UPDATE managed_databases SET site_id=NULL WHERE name=?1",
                    [name],
                )?;
                Ok(json!({"restartRequired":self.restart_required(site.as_deref())}))
            }
            _ => bail!("Unknown MySQL command."),
        }
    }
}
pub fn validate_backup_filename(file: &str) -> Result<()> {
    if file.is_empty()
        || !file.ends_with(".sql")
        || file.contains(['/', '\\', ':'])
        || file.contains("..")
        || file.chars().any(|c| c.is_control())
        || file.ends_with([' ', '.'])
    {
        bail!("Choose a plain .sql filename from this database's backup listing.")
    }
    Ok(())
}
pub fn validate_dump(path: &Path) -> Result<()> {
    let mut file = fs::File::open(path)?;
    let len = file.metadata()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(4096)))?;
    let mut tail = String::new();
    file.read_to_string(&mut tail)?;
    if !tail
        .trim_end()
        .lines()
        .last()
        .is_some_and(|s| s.starts_with("-- Dump completed on "))
    {
        bail!("The backup is truncated or lacks mysqldump's completion comment.")
    }
    Ok(())
}
pub fn data_bytes(path: &Path) -> Result<u64> {
    let mut size = 0;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let meta = fs::symlink_metadata(entry.path())?;
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            size += data_bytes(&entry.path())?
        } else {
            size += meta.len()
        }
    }
    Ok(size)
}
pub fn free_disk(path: &Path) -> Result<u64> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
        let path = fs::canonicalize(path)?;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut available = 0;
        if unsafe {
            GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut available,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(available)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        bail!("Disk measurements require Windows.")
    }
}
/// MySQL requests have their own admission lock and never acquire site/publishing locks.
pub async fn dispatch(engine: &Arc<crate::state::Engine>, method: &str, p: Value) -> Result<Value> {
    let supervisor = engine
        .supervisor
        .as_ref()
        .ok_or_else(|| anyhow!("MySQL requires the process supervisor."))?
        .clone();
    let manager = supervisor.mysql.clone();
    if method == "mysql_status" {
        let running = supervisor.mysql_running().await;
        let measurements = supervisor.mysql_measurements();
        let m = manager.clone();
        return tokio::task::spawn_blocking(move||->Result<Value>{let config=m.config()?;let mut v=json!({"setUp":config.is_some(),"running":running,"healthy":running&&m.sql("SELECT 1;").is_ok(),"lastError":m.store.saved("mysql-last-error")?.unwrap_or(Value::Null)});
    if let Some(c)=config{v["version"]=json!(c.version);v["port"]=json!(c.port);}
    if let Ok(bytes)=data_bytes(&m.store.root.join("mysql/data")){v["dataBytes"]=json!(bytes);}if let Ok(bytes)=free_disk(&m.store.root){v["freeDiskBytes"]=json!(bytes);}
    for(k,value)in measurements.as_object().unwrap(){v[k]=value.clone();}Ok(v)}).await?;
    }
    let _guard = manager
        .operation
        .try_lock()
        .map_err(|_| anyhow!("A MySQL operation is already in progress."))?;
    let db = p["name"]
        .as_str()
        .or_else(|| p["database"].as_str())
        .map(str::to_owned);
    // Explicit reveal and read-only requests never include returned data in progress events.
    manager.progress(method, db.as_deref(), "running", "MySQL operation started.");
    let result=async {
 match method {
 "mysql_start"=>{supervisor.mysql_start().await.map_err(|e|anyhow!(e))?;Ok(Value::Null)},
 "mysql_stop"=>{supervisor.mysql_stop().await.map_err(|e|anyhow!(e))?;Ok(Value::Null)},
 "mysql_restart"=>{supervisor.mysql_stop().await.map_err(|e|anyhow!(e))?;supervisor.mysql_start().await.map_err(|e|anyhow!(e))?;Ok(Value::Null)},
 "restore_database"=>{
 let name=p["name"].as_str().ok_or_else(||anyhow!("Missing database name."))?.to_owned();let file=p["file"].as_str().ok_or_else(||anyhow!("Missing backup filename."))?.to_owned();
 manager.managed(&name)?;if p["confirmName"].as_str()!=Some(&name){bail!("Type the database name exactly to confirm restore.")}
 let m=manager.clone();let n=name.clone();let f=file.clone();let safety=tokio::task::spawn_blocking(move||->Result<Value>{validate_dump(&m.backup_file(&n,&f)?)?;m.backup(&n)}).await??;
 let safety_file=safety["file"].as_str().ok_or_else(||anyhow!("Safety backup filename is missing."))?.to_owned();
 manager.progress(method,Some(&name),"running",&format!("Safety backup created: {safety_file}"));
 let mut stopped=Vec::new();let mut stop_error=None;
 for site in manager.linked_sites(&name)?{if engine.store.site(&site).is_ok_and(|s|matches!(s.status.as_str(),"running"|"starting"|"error")){
 match engine.runtime.stop_site(&site).await{Ok(())=>stopped.push(site),Err(e)=>{stop_error=Some(e);break}}}}
 let outcome=if let Some(error)=stop_error{Err(anyhow!("Could not stop a linked website: {error}. Safety backup: {safety_file}."))}else{
 let m=manager.clone();let n=name.clone();let f=file.clone();let safety_name=safety_file.clone();tokio::task::spawn_blocking(move||->Result<Value>{
 match m.restore_file(&n,&f){Ok(())=>Ok(json!({"safetyBackup":safety_name,"restored":true})),Err(error)=>{
 let recovery=m.restore_file(&n,&safety_name);bail!("Restore failed: {}. Safety backup: {safety_name}. Recovery: {}.",m.redact(&error.to_string()),match recovery{Ok(())=>"succeeded".into(),Err(e)=>format!("failed: {}",m.redact(&e.to_string()))})}}
 }).await?};
 let mut restart_errors=Vec::new();for site in stopped{if let Err(e)=engine.runtime.start_site(&site).await{restart_errors.push(format!("{site}: {e}"));}}
 if !restart_errors.is_empty(){bail!("Restore outcome: {}. Safety backup: {safety_file}. Website restart failed: {}",match &outcome{Ok(_)=>"succeeded".into(),Err(e)=>e.to_string()},restart_errors.join("; "))}outcome
 },
 _=>{let running=supervisor.mysql_running().await;let m=manager.clone();let method=method.to_owned();let setup=method=="mysql_setup";let value=tokio::task::spawn_blocking(move||m.request(&method,&p,running)).await??;if setup{supervisor.mysql_start().await.map_err(|e|anyhow!(e))?;}Ok(value)}
 }
 }.await;
    manager.progress(
        method,
        db.as_deref(),
        if result.is_ok() { "done" } else { "failed" },
        if result.is_ok() {
            "MySQL operation completed."
        } else {
            "MySQL operation failed. See the request error for details."
        },
    );
    result.map_err(|e| anyhow!(manager.redact(&e.to_string())))
}
pub fn is_command(method: &str) -> bool {
    matches!(
        method,
        "mysql_setup"
            | "mysql_status"
            | "mysql_start"
            | "mysql_stop"
            | "mysql_restart"
            | "list_databases"
            | "create_database"
            | "delete_database"
            | "create_db_user"
            | "delete_db_user"
            | "rotate_db_password"
            | "reveal_db_password"
            | "database_connection"
            | "test_connection"
            | "backup_database"
            | "list_backups"
            | "delete_backup"
            | "restore_database"
            | "link_database"
            | "unlink_database"
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Memory(std::sync::Mutex<std::collections::HashMap<String, String>>);
    impl SecretStore for Memory {
        fn set(
            &self,
            k: &str,
            v: &str,
        ) -> std::result::Result<(), hostify_core::secrets::SecretError> {
            self.0.lock().unwrap().insert(k.into(), v.into());
            Ok(())
        }
        fn get(
            &self,
            k: &str,
        ) -> std::result::Result<Option<String>, hostify_core::secrets::SecretError> {
            Ok(self.0.lock().unwrap().get(k).cloned())
        }
        fn delete(&self, k: &str) -> std::result::Result<(), hostify_core::secrets::SecretError> {
            self.0.lock().unwrap().remove(k);
            Ok(())
        }
    }
    struct TestSealer;
    impl Sealer for TestSealer {
        fn seal(&self, v: &[u8]) -> Result<Vec<u8>> {
            Ok(v.iter().map(|b| b ^ 0xa5).collect())
        }
        fn open(&self, v: &[u8]) -> Result<Vec<u8>> {
            self.seal(v)
        }
    }
    fn fixture() -> (tempfile::TempDir, Arc<Manager>) {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path().into()).unwrap();
        let m = Manager::new(store, Arc::new(Memory::default()), Arc::new(TestSealer));
        (tmp, m)
    }
    #[test]
    fn names() {
        for s in [
            "", "a'", "a`", "a;", "a b", "a\n", "é", "中文", "A", "1a", "_a",
        ] {
            assert!(validate_database(s).is_err(), "{s}");
            assert!(validate_user(s).is_err(), "{s}");
        }
        for s in ["mysql", "sys", "information_schema", "performance_schema"] {
            assert!(validate_database(s).is_err())
        }
        assert!(validate_database(&"a".repeat(48)).is_ok());
        assert!(validate_database(&"a".repeat(49)).is_err());
        assert!(validate_user(&"a".repeat(32)).is_ok());
        assert!(validate_user(&"a".repeat(33)).is_err());
        assert!(validate_database("a_b2").is_ok());
    }
    #[test]
    fn ini() {
        let s = generate_ini(Path::new(r"C:\mysql"), Path::new(r"D:\data"), 3307).unwrap();
        for expected in [
            "basedir=\"C:/mysql\"",
            "port=3307",
            "bind-address=127.0.0.1",
            "mysqlx=OFF",
            "local_infile=OFF",
            "secure-file-priv=NULL",
            "performance_schema=OFF",
        ] {
            assert!(s.contains(expected), "{s}");
        }
        assert!(generate_ini(Path::new("bad\"path"), Path::new("d"), 3307).is_err());
        assert!(generate_ini(Path::new("x"), Path::new("d\nx"), 3307).is_err());
    }
    #[test]
    fn conflicts_are_atomic_and_only_matching_injections_removed() {
        let (_tmp, m) = fixture();
        m.store.db().unwrap().execute_batch("INSERT INTO websites(id,name,source_path,local_port,created_at,updated_at) VALUES('s','site','x',9000,'now','now');INSERT INTO managed_databases(name,site_id,created_at) VALUES('app','s','now');").unwrap();
        let password = "A".repeat(32);
        let sealed = m.sealer.seal(password.as_bytes()).unwrap();
        m.store
            .db()
            .unwrap()
            .execute(
                "INSERT INTO managed_db_users VALUES('app_app','app',?1,'now')",
                [sealed],
            )
            .unwrap();
        fs::create_dir_all(m.store.root.join("mysql")).unwrap();
        atomic(
            &m.store.root.join("mysql/.hostify-initialized"),
            r#"{"version":"8.4.10","port":3307,"enabled":true}"#,
        )
        .unwrap();
        let values = m.env_values("app", "app_app", &password).unwrap();
        m.inject("s", &values, false).unwrap();
        m.store
            .db()
            .unwrap()
            .execute(
                "UPDATE env_vars SET plain_value='edited' WHERE name='DB_HOST'",
                [],
            )
            .unwrap();
        let before = m
            .store
            .db()
            .unwrap()
            .query_row("SELECT count(*) FROM env_vars", [], |r| r.get::<_, u64>(0))
            .unwrap();
        let error = m.inject("s", &values, false).unwrap_err().to_string();
        assert!(error.contains("DB_HOST"));
        assert!(!error.contains(&password));
        assert_eq!(
            m.store
                .db()
                .unwrap()
                .query_row("SELECT count(*) FROM env_vars", [], |r| r.get::<_, u64>(0))
                .unwrap(),
            before
        );
        m.remove_injected("app").unwrap();
        let db = m.store.db().unwrap();
        assert_eq!(
            m.env_read(&db, "s", "DB_HOST").unwrap().as_deref(),
            Some("edited")
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM env_vars", [], |r| r.get::<_, u64>(0))
                .unwrap(),
            1
        );
    }
    #[test]
    fn traversal_and_unmanaged_refusal() {
        let (_tmp, m) = fixture();
        for f in [
            "../x.sql",
            r"..\x.sql",
            r"C:\x.sql",
            "x/y.sql",
            "x:ads.sql",
            "x.sql.",
            "x.sql ",
            "",
            "x.partial",
        ] {
            assert!(validate_backup_filename(f).is_err(), "{f}");
        }
        assert!(validate_backup_filename("backup.sql").is_ok());
        assert!(
            m.delete_database("outside", "outside")
                .unwrap_err()
                .to_string()
                .contains("did not create")
        );
        assert!(
            m.request(
                "delete_backup",
                &json!({"name":"outside","file":"../x.sql"}),
                false
            )
            .is_err()
        );
    }
    #[test]
    fn truncated_dump() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("dump.sql");
        fs::write(&path, "CREATE TABLE t(i INT);\n").unwrap();
        assert!(validate_dump(&path).is_err());
        fs::write(&path, "-- Dump completed on 2026-10-31 12:00:00\n").unwrap();
        validate_dump(&path).unwrap();
    }
    #[cfg(windows)]
    #[test]
    fn restricted_acl_is_read_back() {
        use windows_sys::Win32::{
            Foundation::LocalFree,
            Security::{Authorization::*, *},
        };
        let tmp = tempfile::tempdir().unwrap();
        restrict(tmp.path(), true).unwrap();
        unsafe {
            let path = crate::windows::wide(tmp.path().to_str().unwrap());
            let mut sd = std::ptr::null_mut();
            assert_eq!(
                GetNamedSecurityInfoW(
                    path.as_ptr(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut sd
                ),
                0
            );
            let mut control = 0;
            let mut revision = 0;
            assert_ne!(
                GetSecurityDescriptorControl(sd, &mut control, &mut revision),
                0
            );
            assert_ne!(control & SE_DACL_PROTECTED, 0);
            let mut text = std::ptr::null_mut();
            assert_ne!(
                ConvertSecurityDescriptorToStringSecurityDescriptorW(
                    sd,
                    1,
                    DACL_SECURITY_INFORMATION,
                    &mut text,
                    std::ptr::null_mut()
                ),
                0
            );
            let mut n = 0;
            while *text.add(n) != 0 {
                n += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(text, n));
            assert!(s.contains(&crate::windows::sid().unwrap()));
            assert!(s.contains(";;;SY)"));
            assert_eq!(s.matches("(A;").count(), 2);
            LocalFree(text.cast());
            LocalFree(sd);
        }
    }
    #[cfg(windows)]
    #[test]
    fn temp_guard_removes_credentials_on_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path;
        {
            let f = restricted_temp(tmp.path(), "[client]\npassword=secret\n").unwrap();
            path = f.0.clone();
            assert!(path.exists());
        }
        assert!(!path.exists());
    }
}
