//! Hostify Network registry for a **future centralized control plane**.
//!
//! This module intentionally exposes no network listener, Cloudflare token,
//! tunnel credentials or desktop IPC. An authenticated API layer must enforce
//! administrator permissions and verify an agent's cryptographic proof of
//! possession before invoking these operations.
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum NetworkError {
    #[error("invalid tenant, device, subdomain or public-key input")]
    Invalid,
    #[error("tenant or device is unavailable or revoked")]
    Unauthorized,
    #[error("enrollment invitation is expired or already consumed")]
    InvalidInvitation,
    #[error("tenant quota has been reached")]
    Quota,
    #[error("hostname already reserved")]
    HostnameTaken,
    #[error("the requested resource does not belong to this tenant")]
    Ownership,
    #[error(transparent)]
    Storage(#[from] rusqlite::Error),
}

type Result<T> = std::result::Result<T, NetworkError>;
const DEFAULT_DEVICES: i64 = 3;
const DEFAULT_HOSTNAMES: i64 = 10;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tenant {
    pub id: String,
    pub name: String,
    pub max_devices: i64,
    pub max_hostnames: i64,
}
#[derive(Clone, Debug)]
pub struct Invitation {
    /// Display only once. Only the SHA-256 digest is stored in SQLite.
    pub token: String,
    pub expires_unix: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Agent {
    pub id: String,
    pub tenant_id: String,
    pub public_key_hex: String,
    pub created_at: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Route {
    pub id: String,
    pub tenant_id: String,
    pub agent_id: String,
    pub site_id: String,
    pub hostname: String,
}

/// Only the central server may instantiate this registry with its own DB.
/// Never use this registry as a substitute for authenticated API authorization.
pub struct Registry {
    db: Connection,
    base_domain: String,
}
fn require_uuid(id: &str) -> Result<()> {
    Uuid::parse_str(id).map(|_| ()).map_err(|_| NetworkError::Invalid)
}
fn token_hash(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}
fn valid_public_key(key: &str) -> Result<String> {
    let bytes = hex::decode(key).map_err(|_| NetworkError::Invalid)?;
    if bytes.len()!=32 || bytes.iter().all(|b|*b==0) { return Err(NetworkError::Invalid); }
    Ok(hex::encode(bytes))
}
pub fn normalize_label(label: &str) -> Result<String> {
    let label = label.trim().to_ascii_lowercase();
    if label.len()<3 || label.len()>63 ||
        label.starts_with('-') || label.ends_with('-') ||
        !label.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c==b'-') {
        return Err(NetworkError::Invalid);
    }
    // Keep system/auth and common mail endpoints for platform infrastructure.
    if matches!(label.as_str(), "www"|"api"|"admin"|"root"|"login"|"auth"|
        "mail"|"smtp"|"imap"|"ftp"|"ssh"|"support"|"billing"|"app"|
        "assets"|"cdn"|"static"|"status"|"system"|"internal"|"localhost") ||
        label.starts_with("xn--") {
        return Err(NetworkError::Invalid);
    }
    Ok(label)
}
fn validate_domain(domain:&str)->Result<String>{
    let domain=domain.trim().to_ascii_lowercase();
    if domain.len()>253 || !domain.contains('.') || domain.ends_with('.') ||
        domain.split('.').any(|label|label.is_empty() || label.len()>63 ||
            label.starts_with('-') || label.ends_with('-') ||
            !label.bytes().all(|b|b.is_ascii_lowercase()||b.is_ascii_digit()||b==b'-')) {
        return Err(NetworkError::Invalid);
    }
    Ok(domain)
}
impl Registry {
    pub fn open(path: &Path, base_domain: &str) -> Result<Self> {
        let db=Connection::open(path)?;
        Self::initialize(db,base_domain)
    }
    pub fn memory(base_domain: &str) -> Result<Self> {
        Self::initialize(Connection::open_in_memory()?,base_domain)
    }
    fn initialize(db: Connection, base_domain: &str) -> Result<Self> {
        let base_domain=validate_domain(base_domain)?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.execute_batch("
            PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS network_tenants(
                id TEXT PRIMARY KEY, name TEXT NOT NULL, state TEXT NOT NULL DEFAULT 'active'
                    CHECK(state IN ('active','suspended')),
                max_devices INTEGER NOT NULL, max_hostnames INTEGER NOT NULL, created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS network_invitations(
                id TEXT PRIMARY KEY, tenant_id TEXT NOT NULL REFERENCES network_tenants(id),
                token_hash TEXT NOT NULL UNIQUE, expires_unix INTEGER NOT NULL,
                consumed_at TEXT, created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS network_agents(
                id TEXT PRIMARY KEY, tenant_id TEXT NOT NULL REFERENCES network_tenants(id),
                public_key_hex TEXT NOT NULL UNIQUE, created_at TEXT NOT NULL, revoked_at TEXT
            );
            CREATE TABLE IF NOT EXISTS network_routes(
                id TEXT PRIMARY KEY, tenant_id TEXT NOT NULL REFERENCES network_tenants(id),
                agent_id TEXT NOT NULL REFERENCES network_agents(id), site_id TEXT NOT NULL,
                hostname TEXT NOT NULL UNIQUE, state TEXT NOT NULL DEFAULT 'active'
                    CHECK(state IN ('active','disabled')), created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS network_audit(
                id INTEGER PRIMARY KEY AUTOINCREMENT, tenant_id TEXT NOT NULL,
                ts TEXT NOT NULL, action TEXT NOT NULL, subject_id TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_network_agents_tenant
                ON network_agents(tenant_id,revoked_at);
            CREATE INDEX IF NOT EXISTS idx_network_routes_tenant
                ON network_routes(tenant_id,state);
        ")?;
        Ok(Self{db,base_domain})
    }
    fn ensure_tenant(&self, id:&str)->Result<()> {
        require_uuid(id)?;
        let active:Option<String>=self.db.query_row(
            "SELECT state FROM network_tenants WHERE id=?1",[id],|r|r.get(0)).optional()?;
        if active.as_deref()!=Some("active") {return Err(NetworkError::Unauthorized);}
        Ok(())
    }
    pub fn create_tenant(&mut self,name:&str)->Result<Tenant>{
        let name=name.trim();
        if name.is_empty() || name.chars().count()>80 {return Err(NetworkError::Invalid);}
        let tenant=Tenant{
            id:Uuid::new_v4().to_string(),name:name.into(),
            max_devices:DEFAULT_DEVICES,max_hostnames:DEFAULT_HOSTNAMES
        };
        self.db.execute(
            "INSERT INTO network_tenants(id,name,max_devices,max_hostnames,created_at)
                VALUES(?1,?2,?3,?4,?5)",
            params![tenant.id,tenant.name,tenant.max_devices,tenant.max_hostnames,Utc::now().to_rfc3339()]
        )?;
        self.audit(&tenant.id,"tenant_created",&tenant.id)?;
        Ok(tenant)
    }
    fn audit(&self,tenant:&str,action:&str,subject:&str)->Result<()> {
        self.db.execute("INSERT INTO network_audit(tenant_id,ts,action,subject_id)
            VALUES(?1,?2,?3,?4)",params![tenant,Utc::now().to_rfc3339(),action,subject])?;
        Ok(())
    }
    /// Call only from authenticated tenant-owner/administrator context.
    pub fn issue_invitation(&mut self, tenant:&str, ttl_seconds:i64)->Result<Invitation>{
        self.ensure_tenant(tenant)?;
        if !(60..=3600).contains(&ttl_seconds){return Err(NetworkError::Invalid);}
        // 2 independent OS-CSPRNG-backed UUIDv4 values provide ~244 bits
        // of unpredictable invitation material.
        let token=format!("{}{}",Uuid::new_v4().simple(),Uuid::new_v4().simple());
        let expires_unix=Utc::now().timestamp()+ttl_seconds;
        self.db.execute("INSERT INTO network_invitations(id,tenant_id,token_hash,expires_unix,created_at)
            VALUES(?1,?2,?3,?4,?5)",
            params![Uuid::new_v4().to_string(),tenant,token_hash(&token),expires_unix,Utc::now().to_rfc3339()]
        )?;
        self.audit(tenant,"enrollment_invitation_issued","agent-pending")?;
        Ok(Invitation{token,expires_unix})
    }
    /// The invitation proves temporary enrollment authorization only. Before
    /// enabling routing, the gateway must prove possession of the registered
    /// public key using authenticated mutual TLS or signed challenges.
    pub fn enroll_agent(&mut self,token:&str,public_key_hex:&str)->Result<Agent>{
        if token.len()!=64 || !token.bytes().all(|b|b.is_ascii_hexdigit()) {
            return Err(NetworkError::InvalidInvitation);
        }
        let key=valid_public_key(public_key_hex)?;
        let now=Utc::now().timestamp();
        let tx=self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let entry:Option<(String,String,i64,Option<String>)>=tx.query_row(
            "SELECT id,tenant_id,expires_unix,consumed_at FROM network_invitations
                WHERE token_hash=?1",[token_hash(token)],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))
        ).optional()?;
        let (inv_id,tenant,expires,used)=entry.ok_or(NetworkError::InvalidInvitation)?;
        if used.is_some()||expires<=now{return Err(NetworkError::InvalidInvitation);}
        let quota:Option<(String,i64)>=tx.query_row(
            "SELECT state,max_devices FROM network_tenants WHERE id=?1",[&tenant],
            |r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        let (state,max_devices)=quota.ok_or(NetworkError::Unauthorized)?;
        if state!="active"{return Err(NetworkError::Unauthorized);}
        let existing:i64=tx.query_row(
            "SELECT COUNT(*) FROM network_agents WHERE tenant_id=?1 AND revoked_at IS NULL",
            [&tenant],|r|r.get(0))?;
        if existing>=max_devices {return Err(NetworkError::Quota);}
        let id=Uuid::new_v4().to_string();
        let created_at=Utc::now().to_rfc3339();
        let changed=tx.execute(
            "UPDATE network_invitations SET consumed_at=?1
                WHERE id=?2 AND consumed_at IS NULL AND expires_unix>?3",
            params![&created_at,&inv_id,now])?;
        if changed!=1{return Err(NetworkError::InvalidInvitation);}
        tx.execute("INSERT INTO network_agents(id,tenant_id,public_key_hex,created_at)
            VALUES(?1,?2,?3,?4)",params![&id,&tenant,&key,&created_at])?;
        tx.execute("INSERT INTO network_audit(tenant_id,ts,action,subject_id)
            VALUES(?1,?2,'agent_enrolled',?3)",params![&tenant,&created_at,&id])?;
        tx.commit()?;
        Ok(Agent{id,tenant_id:tenant,public_key_hex:key,created_at})
    }
    /// This reserves a globally unique label, but it does NOT create a DNS
    /// record or grant the agent permission to serve traffic.
    pub fn reserve_hostname(&mut self,tenant:&str,agent:&str,site:&str,label:&str)->Result<Route>{
        require_uuid(tenant)?;require_uuid(agent)?;require_uuid(site)?;
        let label=normalize_label(label)?;
        let hostname=format!("{label}.{}",self.base_domain);
        let tx=self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let max_hostnames:i64=tx.query_row(
            "SELECT max_hostnames FROM network_tenants WHERE id=?1 AND state='active'",
            [tenant],|r|r.get(0)).optional()?.ok_or(NetworkError::Unauthorized)?;
        let agent_owner:Option<String>=tx.query_row(
            "SELECT tenant_id FROM network_agents WHERE id=?1 AND revoked_at IS NULL",
            [agent],|r|r.get(0)).optional()?;
        if agent_owner.as_deref()!=Some(tenant){return Err(NetworkError::Ownership);}
        let count:i64=tx.query_row(
            "SELECT COUNT(*) FROM network_routes WHERE tenant_id=?1 AND state='active'",
            [tenant],|r|r.get(0))?;
        if count>=max_hostnames {return Err(NetworkError::Quota);}
        let id=Uuid::new_v4().to_string();
        let created_at=Utc::now().to_rfc3339();
        let insert=tx.execute(
            "INSERT INTO network_routes(id,tenant_id,agent_id,site_id,hostname,created_at)
                VALUES(?1,?2,?3,?4,?5,?6)",
            params![&id,tenant,agent,site,&hostname,&created_at]);
        match insert {
            Err(rusqlite::Error::SqliteFailure(err,_)) if err.code==rusqlite::ErrorCode::ConstraintViolation=>
                return Err(NetworkError::HostnameTaken),
            Err(e)=>return Err(e.into()),
            Ok(_)=>{},
        }
        tx.execute("INSERT INTO network_audit(tenant_id,ts,action,subject_id)
            VALUES(?1,?2,'hostname_reserved',?3)",
            params![tenant,&created_at,&id])?;
        tx.commit()?;
        Ok(Route{id,tenant_id:tenant.into(),agent_id:agent.into(),site_id:site.into(),hostname})
    }
    /// Disabled/revoked agents must never appear as a routable destination.
    pub fn lookup_active_route(&self,hostname:&str)->Result<Option<Route>>{
        let h=hostname.trim().to_ascii_lowercase();
        let route=self.db.query_row(
            "SELECT r.id,r.tenant_id,r.agent_id,r.site_id,r.hostname
                FROM network_routes r
                JOIN network_agents a ON a.id=r.agent_id
                JOIN network_tenants t ON t.id=r.tenant_id
                WHERE r.hostname=?1 AND r.state='active'
                    AND a.revoked_at IS NULL AND t.state='active'",
            [&h],|r|Ok(Route{id:r.get(0)?,tenant_id:r.get(1)?,agent_id:r.get(2)?,
                site_id:r.get(3)?,hostname:r.get(4)?})).optional()?;
        Ok(route)
    }
    /// Revocation is fail-closed and leaves the hostname reserved; never
    /// delete or reassign someone else's DNS record.
    pub fn revoke_agent(&mut self,tenant:&str,agent:&str)->Result<()>{
        require_uuid(tenant)?;require_uuid(agent)?;
        let now=Utc::now().to_rfc3339();
        let tx=self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let affected=tx.execute(
            "UPDATE network_agents SET revoked_at=?1
                WHERE id=?2 AND tenant_id=?3 AND revoked_at IS NULL",
            params![&now,agent,tenant])?;
        if affected!=1{return Err(NetworkError::Ownership);}
        tx.execute("UPDATE network_routes SET state='disabled'
            WHERE agent_id=?1 AND tenant_id=?2",params![agent,tenant])?;
        tx.execute("INSERT INTO network_audit(tenant_id,ts,action,subject_id)
            VALUES(?1,?2,'agent_revoked',?3)",params![tenant,&now,agent])?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    fn key(b:u8)->String {hex::encode([b;32])}
    #[test]
    fn invitations_are_single_use_expiring_hashed_and_tenant_scoped(){
        let mut registry=Registry::memory("example.test").unwrap();
        let t1=registry.create_tenant("Restaurant A").unwrap();
        let t2=registry.create_tenant("Restaurant B").unwrap();
        let token=registry.issue_invitation(&t1.id,600).unwrap();
        let stored: String=registry.db.query_row("SELECT token_hash FROM network_invitations",
            [],|r|r.get(0)).unwrap();
        assert_ne!(stored,token.token);
        assert_eq!(stored,token_hash(&token.token));
        let device=registry.enroll_agent(&token.token,&key(7)).unwrap();
        assert_eq!(device.tenant_id,t1.id);
        assert!(registry.enroll_agent(&token.token,&key(8)).is_err());
        assert!(registry.reserve_hostname(&t2.id,&device.id,&Uuid::new_v4().to_string(),"tenant-b").is_err());
        let expired=registry.issue_invitation(&t2.id,60).unwrap();
        registry.db.execute("UPDATE network_invitations SET expires_unix=0
            WHERE token_hash=?1",[token_hash(&expired.token)]).unwrap();
        assert!(registry.enroll_agent(&expired.token,&key(8)).is_err());
    }
    #[test]
    fn hostname_is_unique_and_revocation_preserves_reservation(){
        let mut registry=Registry::memory("example.test").unwrap();
        let a=registry.create_tenant("Alpha").unwrap();
        let b=registry.create_tenant("Beta").unwrap();
        let ia=registry.issue_invitation(&a.id,60).unwrap();
        let ib=registry.issue_invitation(&b.id,60).unwrap();
        let da=registry.enroll_agent(&ia.token,&key(2)).unwrap();
        let db=registry.enroll_agent(&ib.token,&key(3)).unwrap();
        let route=registry.reserve_hostname(&a.id,&da.id,&Uuid::new_v4().to_string(),"ALPHA-APP").unwrap();
        assert_eq!(route.hostname,"alpha-app.example.test");
        assert!(registry.reserve_hostname(&b.id,&db.id,&Uuid::new_v4().to_string(),"alpha-app").is_err());
        assert!(registry.lookup_active_route(&route.hostname).unwrap().is_some());
        assert!(registry.revoke_agent(&b.id,&da.id).is_err());
        registry.revoke_agent(&a.id,&da.id).unwrap();
        assert!(registry.lookup_active_route(&route.hostname).unwrap().is_none());
        assert!(registry.reserve_hostname(&b.id,&db.id,&Uuid::new_v4().to_string(),"alpha-app").is_err());
    }
    #[test]
    fn reserved_names_and_invalid_labels_are_rejected(){
        for name in ["api","admin","www","mail","root","xn--test","a","-foo","foo.","some.name","bad_name"] {
            assert!(normalize_label(name).is_err(),"{name}");
        }
        assert_eq!(normalize_label(" My-Site ").unwrap(),"my-site");
    }
    #[test]
    fn registry_is_durable_across_reopen_and_stores_no_raw_token(){
        let dir=tempfile::tempdir().unwrap();
        let path=dir.path().join("network.db");
        let (tenant,agent,hostname)={
            let mut r=Registry::open(&path,"example.test").unwrap();
            let t=r.create_tenant("Test").unwrap();
            let invite=r.issue_invitation(&t.id,300).unwrap();
            let d=r.enroll_agent(&invite.token,&key(11)).unwrap();
            let route=r.reserve_hostname(&t.id,&d.id,&Uuid::new_v4().to_string(),"demo-test").unwrap();
            (t.id,d.id,route.hostname)
        };
        let mut r=Registry::open(&path,"example.test").unwrap();
        assert_eq!(r.lookup_active_route(&hostname).unwrap().unwrap().tenant_id,tenant);
        r.revoke_agent(&tenant,&agent).unwrap();
        assert!(r.lookup_active_route(&hostname).unwrap().is_none());
    }
}
