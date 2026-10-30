use rusqlite::{Connection, Result};

/// Apply migrations in one transaction. A newer schema is never silently downgraded.
pub fn migrate(connection: &mut Connection) -> Result<()> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > 2 { return Err(rusqlite::Error::InvalidQuery); }
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
        let tx=connection.transaction()?;
        tx.execute_batch("UPDATE deployments SET state='published',snapshot_json=json_set(snapshot_json,'$.state','published') WHERE state='live'; PRAGMA user_version=2;")?;
        tx.commit()?;
    }
    connection.pragma_update(None, "foreign_keys", "ON")?;
    Ok(())
}
#[cfg(test)] mod tests {
    #[test] fn migration_is_idempotent() {
        let mut db = rusqlite::Connection::open_in_memory().expect("memory database");
        super::migrate(&mut db).expect("first migration"); super::migrate(&mut db).expect("second migration");
        let count: i64 = db.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('websites','domains','deployments','tunnel_connections','application_settings','activity_logs')", [], |r| r.get(0)).expect("tables");
        assert_eq!(count, 6);
    }
}

#[cfg(test)]
mod legacy_migration {
    #[test]
    fn live_rows_and_snapshots_become_published() {
        let mut db=rusqlite::Connection::open_in_memory().unwrap();super::migrate(&mut db).unwrap();
        db.execute("INSERT INTO websites(id,name,source_path,local_port,created_at,updated_at) VALUES('s','s','x',9000,'now','now')",[]).unwrap();
        db.execute("INSERT INTO deployments(id,site_id,state,snapshot_json,started_at) VALUES('d','s','live','{\"state\":\"live\"}','now')",[]).unwrap();
        db.pragma_update(None,"user_version",1).unwrap();super::migrate(&mut db).unwrap();
        let (state,snapshot):(String,String)=db.query_row("SELECT state,snapshot_json FROM deployments",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!(state,"published");assert_eq!(serde_json::from_str::<serde_json::Value>(&snapshot).unwrap()["state"],"published");
    }
}
