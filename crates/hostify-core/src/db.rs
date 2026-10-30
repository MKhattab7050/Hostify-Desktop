use rusqlite::{Connection, Result};

/// Apply migrations in one transaction. A newer schema is never silently downgraded.
pub fn migrate(connection: &mut Connection) -> Result<()> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > 5 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    if version == 0 {
        let tx = connection.transaction()?;
        tx.execute_batch("CREATE TABLE websites (id TEXT PRIMARY KEY, name TEXT NOT NULL, source_path TEXT NOT NULL, local_port INTEGER NOT NULL UNIQUE, auto_start INTEGER NOT NULL DEFAULT 1, file_count INTEGER NOT NULL DEFAULT 0, size_bytes INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
CREATE TABLE domains (id TEXT PRIMARY KEY, hostname TEXT NOT NULL UNIQUE, zone_name TEXT NOT NULL, zone_id TEXT NOT NULL, site_id TEXT REFERENCES websites(id), dns_status TEXT NOT NULL, https_verified INTEGER NOT NULL DEFAULT 0);
CREATE TABLE deployments (id TEXT PRIMARY KEY, site_id TEXT NOT NULL REFERENCES websites(id), state TEXT NOT NULL, snapshot_json TEXT NOT NULL, created_resources_json TEXT NOT NULL DEFAULT '[]', started_at TEXT NOT NULL, finished_at TEXT);
CREATE TABLE tunnel_connections (id TEXT PRIMARY KEY, name TEXT NOT NULL, account_id TEXT NOT NULL, credential_ref TEXT, state TEXT NOT NULL);
CREATE TABLE application_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE activity_logs (id INTEGER PRIMARY KEY, ts TEXT NOT NULL, level TEXT NOT NULL, site_id TEXT, message TEXT NOT NULL);
PRAGMA user_version = 1;")?;
        tx.commit()?;
    }
    if version < 2 {
        let tx = connection.transaction()?;
        tx.execute_batch("UPDATE deployments SET state='published',snapshot_json=json_set(snapshot_json,'$.state','published') WHERE state='live'; PRAGMA user_version=2;")?;
        tx.commit()?;
    }
    if version < 3 {
        // Older engines wrote internal wording to the activity feed and a row for every routine start.
        let tx = connection.transaction()?;
        tx.execute_batch("DELETE FROM activity_logs WHERE message='Child health check passed';
UPDATE activity_logs SET message='The website server isn''t responding. Hostify keeps checking.' WHERE message='Child health check failed';
UPDATE activity_logs SET message='Website started' WHERE message='start_site';
UPDATE activity_logs SET message='Website stopped' WHERE message='stop_site';
UPDATE activity_logs SET message='Website restarted' WHERE message='restart_site';
UPDATE activity_logs SET message='Hostify engine started' WHERE message LIKE 'Hostify engine started (data:%';
PRAGMA user_version=3;")?;
        tx.commit()?;
    }
    if version < 4 {
        let tx = connection.transaction()?;
        tx.execute_batch(
            "ALTER TABLE websites ADD COLUMN kind TEXT NOT NULL DEFAULT 'static';
ALTER TABLE websites ADD COLUMN runtime_version TEXT;
ALTER TABLE websites ADD COLUMN app_config_json TEXT NOT NULL DEFAULT '{}';
CREATE TABLE env_vars (
 site_id TEXT NOT NULL REFERENCES websites(id) ON DELETE CASCADE,
 name TEXT NOT NULL, secret INTEGER NOT NULL DEFAULT 0,
 plain_value TEXT, sealed_value BLOB, updated_at TEXT NOT NULL,
 PRIMARY KEY(site_id,name),
 CHECK ((plain_value IS NOT NULL) != (sealed_value IS NOT NULL)),
 CHECK ((secret=0 AND plain_value IS NOT NULL) OR (secret=1 AND sealed_value IS NOT NULL))
);
PRAGMA user_version=4;",
        )?;
        tx.commit()?;
    }
    if version < 5 {
        let tx = connection.transaction()?;
        tx.execute_batch("CREATE TABLE managed_databases (
 name TEXT PRIMARY KEY, site_id TEXT, created_at TEXT NOT NULL,
 last_backup_at TEXT, last_backup_path TEXT, last_backup_bytes INTEGER
);
CREATE TABLE managed_db_users (
 user_name TEXT PRIMARY KEY,
 database_name TEXT NOT NULL REFERENCES managed_databases(name) ON DELETE CASCADE,
 sealed_password BLOB NOT NULL, created_at TEXT NOT NULL
);
PRAGMA user_version=5;")?;
        tx.commit()?;
    }
    connection.pragma_update(None, "foreign_keys", "ON")?;
    Ok(())
}
#[cfg(test)]
mod tests {
    #[test]
    fn migration_is_idempotent() {
        let mut db = rusqlite::Connection::open_in_memory().expect("memory database");
        super::migrate(&mut db).expect("first migration");
        super::migrate(&mut db).expect("second migration");
        let count: i64 = db.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('websites','domains','deployments','tunnel_connections','application_settings','activity_logs')", [], |r| r.get(0)).expect("tables");
        assert_eq!(count, 6);
    }
}

#[cfg(test)]
mod legacy_migration {
    #[test]
    fn old_activity_wording_is_rewritten_and_routine_rows_removed() {
        let mut db = rusqlite::Connection::open_in_memory().unwrap();
        super::migrate(&mut db).unwrap();
        for message in [
            "Child health check passed",
            "Child health check failed",
            "start_site",
            "Hostify engine started (data: C:\\x)",
            "Website files imported",
        ] {
            db.execute(
                "INSERT INTO activity_logs(ts,level,message) VALUES('now','info',?1)",
                [message],
            )
            .unwrap();
        }
        db.execute_batch("DROP TABLE managed_db_users; DROP TABLE managed_databases; DROP TABLE env_vars; ALTER TABLE websites DROP COLUMN kind; ALTER TABLE websites DROP COLUMN runtime_version; ALTER TABLE websites DROP COLUMN app_config_json;").unwrap();
        db.pragma_update(None, "user_version", 2).unwrap();
        super::migrate(&mut db).unwrap();
        let mut query = db
            .prepare("SELECT message FROM activity_logs ORDER BY id")
            .unwrap();
        let rows: Vec<String> = query
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(
            rows,
            [
                "The website server isn't responding. Hostify keeps checking.",
                "Website started",
                "Hostify engine started",
                "Website files imported"
            ]
        );
    }
    #[test]
    fn live_rows_and_snapshots_become_published() {
        let mut db = rusqlite::Connection::open_in_memory().unwrap();
        super::migrate(&mut db).unwrap();
        db.execute("INSERT INTO websites(id,name,source_path,local_port,created_at,updated_at) VALUES('s','s','x',9000,'now','now')",[]).unwrap();
        db.execute("INSERT INTO deployments(id,site_id,state,snapshot_json,started_at) VALUES('d','s','live','{\"state\":\"live\"}','now')",[]).unwrap();
        db.execute_batch("DROP TABLE managed_db_users; DROP TABLE managed_databases; DROP TABLE env_vars; ALTER TABLE websites DROP COLUMN kind; ALTER TABLE websites DROP COLUMN runtime_version; ALTER TABLE websites DROP COLUMN app_config_json;").unwrap();
        db.pragma_update(None, "user_version", 1).unwrap();
        super::migrate(&mut db).unwrap();
        let (state, snapshot): (String, String) = db
            .query_row("SELECT state,snapshot_json FROM deployments", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(state, "published");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&snapshot).unwrap()["state"],
            "published"
        );
    }
}

/// Unknown or malformed resource journals are retained for manual recovery.
pub fn prune(db: &Connection) -> Result<()> {
    db.execute("DELETE FROM activity_logs WHERE id NOT IN (SELECT id FROM activity_logs ORDER BY id DESC LIMIT 5000)",[])?;
    let candidates = {
        let mut st=db.prepare("SELECT id,created_resources_json FROM (SELECT id,state,created_resources_json,ROW_NUMBER() OVER (PARTITION BY site_id ORDER BY rowid DESC) AS position FROM deployments) WHERE state IN ('failed','rolled_back') AND position>20")?;
        st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<Result<Vec<_>>>()?
    };
    for (id, raw) in candidates {
        if !resources_needed(&raw) {
            db.execute("DELETE FROM deployments WHERE id=?1", [id])?;
        }
    }
    Ok(())
}
pub fn resources_needed(raw: &str) -> bool {
    let Ok(serde_json::Value::Object(resources)) = serde_json::from_str(&raw) else {
        return true;
    };
    !resources.iter().all(|(key, value)| match key.as_str() {
        "request" | "owner" | "nonce" | "marker_directory" | "previous_config"
        | "desired_config" | "previous_dns" | "dns_update_target" => true,
        "site_started" | "connector_started" | "tunnel_created" | "dns_intent"
        | "ingress_pending" | "rollback_pending" | "dns_update_pending" | "unpublishing" => {
            value == false
        }
        "tunnel_id" | "dns_id" | "tunnel_intent" => value.is_null(),
        _ => false,
    })
}
#[cfg(test)]
mod retention_tests {
    use super::*;
    #[test]
    fn retention_keeps_newest_and_resource_journals() {
        let mut db = Connection::open_in_memory().unwrap();
        migrate(&mut db).unwrap();
        db.execute("INSERT INTO websites(id,name,source_path,local_port,created_at,updated_at) VALUES('s','s','x',9000,'now','now')",[]).unwrap();
        for i in 0..40 {
            let resources = match i {
                0 => r#"{"rollback_pending":true}"#,
                1 => r#"{"tunnel_id":"owned"}"#,
                2 => r#"{"future_resource":true}"#,
                _ => "{}",
            };
            db.execute("INSERT INTO deployments(id,site_id,state,snapshot_json,created_resources_json,started_at) VALUES(?1,'s','failed','{}',?2,'now')",rusqlite::params![i.to_string(),resources]).unwrap();
        }
        for i in 0..5010 {
            db.execute(
                "INSERT INTO activity_logs(ts,level,message) VALUES('now','info',?1)",
                [i.to_string()],
            )
            .unwrap();
        }
        prune(&db).unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM deployments", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            23
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM activity_logs", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            5000
        );
        assert_eq!(
            db.query_row(
                "SELECT message FROM activity_logs ORDER BY id LIMIT 1",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "10"
        );
    }
}
#[cfg(test)]
mod runtime_migration_tests {
    #[test]
    fn v3_sites_and_deployments_survive_v4() {
        let mut db = rusqlite::Connection::open_in_memory().unwrap();
        super::migrate(&mut db).unwrap();
        db.execute_batch("DROP TABLE managed_db_users; DROP TABLE managed_databases; DROP TABLE env_vars; ALTER TABLE websites DROP COLUMN kind; ALTER TABLE websites DROP COLUMN runtime_version; ALTER TABLE websites DROP COLUMN app_config_json; PRAGMA user_version=3; INSERT INTO websites(id,name,source_path,local_port,created_at,updated_at) VALUES('s','legacy','x',9000,'then','then'); INSERT INTO deployments(id,site_id,state,snapshot_json,started_at) VALUES('d','s','published','{}','then');").unwrap();
        super::migrate(&mut db).unwrap();
        let row: (String, String, Option<String>, String) = db
            .query_row(
                "SELECT name,kind,runtime_version,app_config_json FROM websites WHERE id='s'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(row, ("legacy".into(), "static".into(), None, "{}".into()));
        assert_eq!(
            db.query_row("SELECT state FROM deployments WHERE id='d'", [], |r| r
                .get::<_, String>(
                0
            ))
            .unwrap(),
            "published"
        );
        assert!(
            db.execute("INSERT INTO env_vars VALUES('s','X',0,NULL,NULL,'now')", [])
                .is_err()
        );
        assert!(
            db.execute(
                "INSERT INTO env_vars VALUES('s','X',0,'plain',x'01','now')",
                []
            )
            .is_err()
        );
    }
}
#[cfg(test)]
mod mysql_migration_tests {
 #[test]fn v4_data_survives_v5(){let mut db=rusqlite::Connection::open_in_memory().unwrap();super::migrate(&mut db).unwrap();db.execute_batch("DROP TABLE managed_db_users;DROP TABLE managed_databases;PRAGMA user_version=4;INSERT INTO websites(id,name,source_path,local_port,created_at,updated_at) VALUES('s','legacy','x',9000,'then','then');INSERT INTO env_vars VALUES('s','GREETING',0,'hello',NULL,'then');INSERT INTO deployments(id,site_id,state,snapshot_json,started_at) VALUES('d','s','published','{}','then');").unwrap();super::migrate(&mut db).unwrap();assert_eq!(db.pragma_query_value(None,"user_version",|r|r.get::<_,i64>(0)).unwrap(),5);assert_eq!(db.query_row("SELECT plain_value FROM env_vars",[],|r|r.get::<_,String>(0)).unwrap(),"hello");assert_eq!(db.query_row("SELECT state FROM deployments",[],|r|r.get::<_,String>(0)).unwrap(),"published");db.execute_batch("INSERT INTO managed_databases(name,created_at) VALUES('app','now');INSERT INTO managed_db_users VALUES('app_app','app',x'1234','now');DELETE FROM managed_databases;").unwrap();assert_eq!(db.query_row("SELECT count(*) FROM managed_db_users",[],|r|r.get::<_,u64>(0)).unwrap(),0);super::migrate(&mut db).unwrap();}
}
