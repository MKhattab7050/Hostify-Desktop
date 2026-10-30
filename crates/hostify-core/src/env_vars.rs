//! Environment storage; secret values are never included in list or import errors.
use crate::vault;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

pub struct SensitiveString(String);
impl SensitiveString {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl Drop for SensitiveString {
    fn drop(&mut self) {
        unsafe {
            vault::wipe(self.0.as_bytes_mut());
        }
    }
}
#[derive(Serialize)]
pub struct Variable {
    pub name: String,
    pub secret: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}
fn sql(_: rusqlite::Error) -> String {
    "Could not access the website environment variables.".into()
}
fn site(db: &Connection, id: &str) -> Result<(), String> {
    if db
        .query_row("SELECT 1 FROM websites WHERE id=?1", [id], |_| Ok(()))
        .optional()
        .map_err(sql)?
        .is_none()
    {
        return Err("Website not found.".into());
    }
    Ok(())
}
pub fn validate(name: &str, value: &str) -> Result<(), String> {
    let mut bytes = name.bytes();
    if name.len() > 128
        || !bytes
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        || !bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err("Variable names must use letters, numbers and underscores, start with a letter or underscore, and contain at most 128 characters.".into());
    }
    let upper = name.to_ascii_uppercase();
    if upper.starts_with("HOSTIFY_") || matches!(upper.as_str(), "PORT" | "HOST") {
        return Err("This variable name is controlled by Hostify.".into());
    }
    if value.len() > 65536 || value.contains('\0') {
        return Err("Variable values must be at most 64 KiB and cannot contain NUL.".into());
    }
    Ok(())
}
pub fn list(db: &Connection, id: &str) -> Result<Vec<Variable>, String> {
    site(db, id)?;
    db.prepare("SELECT name,secret,CASE WHEN secret=0 THEN plain_value END FROM env_vars WHERE site_id=?1 ORDER BY name").map_err(sql)?
        .query_map([id], |r|Ok(Variable{name:r.get(0)?,secret:r.get(1)?,value:r.get(2)?})).map_err(sql)?.collect::<rusqlite::Result<Vec<_>>>().map_err(sql)
}
pub fn set(db: &Connection, id: &str, name: &str, value: &str, secret: bool) -> Result<(), String> {
    validate(name, value)?;
    site(db, id)?;
    let sealed = if secret {
        Some(vault::seal(value.as_bytes())?)
    } else {
        None
    };
    let plain = if secret { None } else { Some(value) };
    db.execute("INSERT INTO env_vars(site_id,name,secret,plain_value,sealed_value,updated_at) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(site_id,name) DO UPDATE SET secret=excluded.secret,plain_value=excluded.plain_value,sealed_value=excluded.sealed_value,updated_at=excluded.updated_at",params![id,name,secret,plain,sealed,chrono::Utc::now().to_rfc3339()]).map_err(sql)?;
    Ok(())
}
pub fn delete(db: &Connection, id: &str, name: &str) -> Result<(), String> {
    validate(name, "")?;
    site(db, id)?;
    db.execute(
        "DELETE FROM env_vars WHERE site_id=?1 AND name=?2",
        params![id, name],
    )
    .map_err(sql)?;
    Ok(())
}
pub fn reveal(db: &Connection, id: &str, name: &str) -> Result<String, String> {
    validate(name, "")?;
    site(db, id)?;
    let (plain, sealed): (Option<String>, Option<Vec<u8>>) = db
        .query_row(
            "SELECT plain_value,sealed_value FROM env_vars WHERE site_id=?1 AND name=?2",
            params![id, name],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(sql)?
        .ok_or("Variable not found.")?;
    if let Some(p) = plain {
        return Ok(p);
    }
    let mut bytes = vault::open(&sealed.unwrap_or_default()).map_err(|_| {
        format!("Could not decrypt variable {name}. Set its value again under this Windows user.")
    })?;
    let value = std::str::from_utf8(&bytes)
        .map(str::to_owned)
        .map_err(|_| format!("Could not decode variable {name}."));
    vault::wipe(&mut bytes);
    value
}
pub fn resolved(db: &Connection, id: &str) -> Result<Vec<(String, SensitiveString)>, String> {
    list(db, id)?
        .into_iter()
        .map(|v| {
            let value = reveal(db, id, &v.name)?;
            Ok((v.name, SensitiveString(value)))
        })
        .collect()
}
pub fn secret_values(db: &Connection, id: &str) -> Result<Vec<SensitiveString>, String> {
    list(db, id)?
        .into_iter()
        .filter(|v| v.secret)
        .map(|v| reveal(db, id, &v.name).map(SensitiveString))
        .collect()
}
fn sensitive(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    [
        "SECRET",
        "TOKEN",
        "PASSWORD",
        "PASS",
        "KEY",
        "PRIVATE",
        "CREDENTIAL",
    ]
    .iter()
    .any(|part| name.contains(part))
}
#[derive(Serialize)]
pub struct ImportResult {
    pub imported: usize,
    pub errors: Vec<LineError>,
}
#[derive(Serialize)]
pub struct LineError {
    pub line: usize,
    pub message: String,
}
// Parsing errors are deliberately constant; source text and values never reach errors.
fn parse(line: &str) -> Result<Option<(String, String)>, String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return Ok(None);
    }
    let line = line
        .strip_prefix("export ")
        .or_else(|| line.strip_prefix("export\t"))
        .unwrap_or(line)
        .trim_start();
    let (name, raw) = line.split_once('=').ok_or("Expected NAME=value.")?;
    let name = name.trim();
    let raw = raw.trim_start();
    let value = if raw.starts_with(['\'', '"']) {
        let quote = raw.chars().next().unwrap();
        let mut chars = raw[1..].char_indices();
        let mut value = String::new();
        let mut end = None;
        while let Some((i, c)) = chars.next() {
            if c == quote {
                end = Some(i + 2);
                break;
            }
            if c == '\\' && quote == '"' {
                let (_, escaped) = chars.next().ok_or("Unclosed quoted value.")?;
                match escaped {
                    'n' => value.push('\n'),
                    'r' => value.push('\r'),
                    't' => value.push('\t'),
                    '"' => value.push('"'),
                    '\\' => value.push('\\'),
                    _ => {
                        value.push('\\');
                        value.push(escaped);
                    }
                }
            } else {
                value.push(c);
            }
        }
        let tail = raw[end.ok_or("Unclosed quoted value.")?..].trim();
        if !tail.is_empty() && !tail.starts_with('#') {
            return Err("Unexpected text after quoted value.".into());
        }
        value
    } else {
        let comment = raw
            .char_indices()
            .find(|(i, c)| *c == '#' && (*i == 0 || raw[..*i].ends_with(char::is_whitespace)))
            .map(|(i, _)| i)
            .unwrap_or(raw.len());
        raw[..comment].trim_end().to_owned()
    };
    validate(name, &value)?;
    Ok(Some((name.to_owned(), value)))
}
pub fn import_dotenv(
    db: &Connection,
    id: &str,
    text: &str,
    mark_secret_names: &[String],
) -> Result<ImportResult, String> {
    site(db, id)?;
    let mut result = ImportResult {
        imported: 0,
        errors: vec![],
    };
    for (index, line) in text.lines().enumerate() {
        let entry = parse(line).and_then(|entry| {
            if let Some((name, value)) = entry {
                let value = SensitiveString(value);
                set(
                    db,
                    id,
                    &name,
                    value.as_str(),
                    sensitive(&name) || mark_secret_names.iter().any(|n| n == &name),
                )?;
                result.imported += 1;
            }
            Ok(())
        });
        if let Err(message) = entry {
            result.errors.push(LineError {
                line: index + 1,
                message,
            });
        }
    }
    Ok(result)
}
pub fn export_nonsecret(db: &Connection, id: &str) -> Result<String, String> {
    let mut text = String::new();
    let mut omitted = vec![];
    for v in list(db, id)? {
        if v.secret {
            omitted.push(v.name);
            continue;
        }
        let value = v
            .value
            .unwrap_or_default()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r");
        text.push_str(&format!("{}=\"{value}\"\n", v.name));
    }
    if !omitted.is_empty() {
        text.push_str(&format!("# Secrets omitted: {}\n", omitted.join(", ")));
    }
    Ok(text)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn db() -> Connection {
        let mut db = Connection::open_in_memory().unwrap();
        crate::db::migrate(&mut db).unwrap();
        db.execute("INSERT INTO websites(id,name,source_path,local_port,created_at,updated_at) VALUES('s','s','x',9000,'now','now')",[]).unwrap();
        db
    }
    #[test]
    fn dotenv_and_rules() {
        let db = db();
        let result=import_dotenv(&db,"s","# hi\nexport NAME='hello # world'\nMULTI=\"a\\nb\" # comment\nBAD=\"private-bad-value\nHOST=bad\n",&[]).unwrap();
        assert_eq!(result.imported, 2);
        assert_eq!(result.errors.len(), 2);
        assert_eq!(result.errors[0].line, 4);
        assert!(
            !serde_json::to_string(&result)
                .unwrap()
                .contains("private-bad-value")
        );
        assert_eq!(reveal(&db, "s", "MULTI").unwrap(), "a\nb");
        let exported = export_nonsecret(&db, "s").unwrap();
        assert_eq!(import_dotenv(&db, "s", &exported, &[]).unwrap().imported, 2);
        for n in ["", "1X", "HOSTIFY_TEST", "PORT", "HOST", "bad-name"] {
            assert!(validate(n, "").is_err());
        }
        assert!(validate("X", &"a".repeat(65537)).is_err());
        assert!(validate("X", "\0").is_err());
    }
    #[cfg(windows)]
    #[test]
    fn secret_is_ciphertext_and_never_listed() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.db");
        let mut db = Connection::open(&path).unwrap();
        crate::db::migrate(&mut db).unwrap();
        db.execute("INSERT INTO websites(id,name,source_path,local_port,created_at,updated_at) VALUES('s','s','x',9000,'now','now')",[]).unwrap();
        let value = "very-private-secret-12345";
        set(&db, "s", "API_TOKEN", value, true).unwrap();
        assert_eq!(reveal(&db, "s", "API_TOKEN").unwrap(), value);
        assert!(
            !serde_json::to_string(&list(&db, "s").unwrap())
                .unwrap()
                .contains(value)
        );
        assert!(
            !std::fs::read(path)
                .unwrap()
                .windows(value.len())
                .any(|w| w == value.as_bytes())
        );
        assert_eq!(
            export_nonsecret(&db, "s").unwrap(),
            "# Secrets omitted: API_TOKEN\n"
        );
        db.execute("UPDATE env_vars SET sealed_value=x'0001'", [])
            .unwrap();
        let err = reveal(&db, "s", "API_TOKEN").unwrap_err();
        assert!(err.contains("API_TOKEN"));
        assert!(!err.contains(value));
    }
}
