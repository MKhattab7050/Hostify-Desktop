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
    #[error("device challenge is invalid, expired, or already consumed")]
    InvalidChallenge,
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
pub(crate) const DEFAULT_DEVICES: i64 = 3;
pub(crate) const DEFAULT_HOSTNAMES: i64 = 10;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tenant {
    pub id: String,
    pub name: String,
    pub max_devices: i64,
    pub max_hostnames: i64,
}
#[derive(Clone)]
pub struct Invitation {
    /// Display only once. Only the SHA-256 digest is stored in SQLite.
    pub token: String,
    pub expires_unix: i64,
}
/// Never reveal one-use credentials in panic contexts, error reports or logs.
impl std::fmt::Debug for Invitation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Invitation")
            .field("token", &"[REDACTED]")
            .field("expires_unix", &self.expires_unix)
            .finish()
    }
}
/// Short-lived challenge returned once to the agent over an authenticated
/// transport. Its raw value is never stored in the registry database.
#[derive(Clone)]
pub struct DeviceChallenge {
    pub value: String,
    pub expires_unix: i64,
}
impl std::fmt::Debug for DeviceChallenge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceChallenge")
            .field("value", &"[REDACTED]")
            .field("expires_unix", &self.expires_unix)
            .finish()
    }
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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    pub id: i64,
    pub tenant_id: String,
    pub ts: String,
    pub action: String,
    pub subject_id: String,
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
    fn initialize(mut db: Connection, base_domain: &str) -> Result<Self> {
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
                hostname TEXT NOT NULL UNIQUE, state TEXT NOT NULL DEFAULT 'reserved'
                    CHECK(state IN ('reserved','active','disabled')), created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS network_route_challenges(
                id TEXT PRIMARY KEY,
                tenant_id TEXT NOT NULL REFERENCES network_tenants(id),
                agent_id TEXT NOT NULL REFERENCES network_agents(id),
                route_id TEXT NOT NULL REFERENCES network_routes(id),
                challenge_hash TEXT NOT NULL UNIQUE,
                expires_unix INTEGER NOT NULL,
                consumed_at TEXT,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS network_agent_proofs(
                route_id TEXT PRIMARY KEY REFERENCES network_routes(id),
                tenant_id TEXT NOT NULL REFERENCES network_tenants(id),
                agent_id TEXT NOT NULL REFERENCES network_agents(id),
                verified_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS network_audit(
                id INTEGER PRIMARY KEY AUTOINCREMENT, tenant_id TEXT NOT NULL,
                ts TEXT NOT NULL, action TEXT NOT NULL, subject_id TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_network_agents_tenant
                ON network_agents(tenant_id,revoked_at);
            CREATE INDEX IF NOT EXISTS idx_network_routes_tenant
                ON network_routes(tenant_id,state);
            CREATE TABLE IF NOT EXISTS network_config(
                key TEXT PRIMARY KEY, value TEXT NOT NULL
            );
        ")?;
        // An existing network registry must not silently change identity if
        // the operator supplies a different domain during a restart.
        let tx=db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let pinned:Option<String>=tx.query_row(
            "SELECT value FROM network_config WHERE key='base_domain'",
            [],|row|row.get(0)).optional()?;
        match pinned {
            Some(existing) if existing!=base_domain=>return Err(NetworkError::Invalid),
            Some(_)=>{},
            None=>{
                // Older databases predate network_config. Check the namespace
                // of every existing reservation before adopting the setting.
                let suffix=format!(".{base_domain}");
                let routes:Vec<String>={
                    let mut stmt=tx.prepare("SELECT hostname FROM network_routes")?;
                    stmt.query_map([],|row|row.get(0))?
                        .collect::<std::result::Result<Vec<_>,_>>()?
                };
                if routes.iter().any(|hostname|
                    !hostname.strip_suffix(&suffix)
                        .is_some_and(|label|normalize_label(label).is_ok())){
                    return Err(NetworkError::Invalid);
                }
                tx.execute("INSERT INTO network_config(key,value)
                    VALUES('base_domain',?1)",[&base_domain])?;
            }
        }
        tx.commit()?;
        Ok(Self{db,base_domain})
    }
    fn ensure_tenant(&self, id:&str)->Result<()> {
        require_uuid(id)?;
        let active:Option<String>=self.db.query_row(
            "SELECT state FROM network_tenants WHERE id=?1",[id],|r|r.get(0)).optional()?;
        if active.as_deref()!=Some("active") {return Err(NetworkError::Unauthorized);}
        Ok(())
    }
    /// Central-only security audit feed. This method enforces the tenant
    /// filter but NOT the calling user's role. The outer service must require
    /// ManageUsers authorization; never expose Registry directly to a client.
    pub fn list_tenant_audit(&self,tenant:&str,limit:i64,offset:i64)->Result<Vec<AuditEntry>>{
        self.ensure_tenant(tenant)?;
        if !(1..=100).contains(&limit) || !(0..=1_000_000).contains(&offset) {
            return Err(NetworkError::Invalid);
        }
        let mut statement=self.db.prepare(
            "SELECT id,tenant_id,ts,action,subject_id FROM network_audit
             WHERE tenant_id=?1 ORDER BY id DESC LIMIT ?2 OFFSET ?3"
        )?;
        let events=statement.query_map(params![tenant,limit,offset],|row|Ok(AuditEntry{
            id:row.get(0)?,tenant_id:row.get(1)?,ts:row.get(2)?,
            action:row.get(3)?,subject_id:row.get(4)?,
        }))?.collect::<std::result::Result<Vec<_>,_>>()?;
        Ok(events)
    }
    /// Central-only, bounded tenant inventory. Call only after authenticating
    /// the requesting tenant; this registry itself is not an authorization API.
    pub fn list_tenant_agents(&self,tenant:&str,limit:i64,offset:i64)->Result<Vec<Agent>>{
        self.ensure_tenant(tenant)?;
        if !(1..=100).contains(&limit) || !(0..=1_000_000).contains(&offset){
            return Err(NetworkError::Invalid);
        }
        let mut stmt=self.db.prepare(
            "SELECT id,tenant_id,public_key_hex,created_at FROM network_agents
             WHERE tenant_id=?1 AND revoked_at IS NULL
             ORDER BY created_at DESC,id DESC LIMIT ?2 OFFSET ?3"
        )?;
        let items=stmt.query_map(params![tenant,limit,offset],|row|Ok(Agent{
            id:row.get(0)?,tenant_id:row.get(1)?,
            public_key_hex:row.get(2)?,created_at:row.get(3)?
        }))?.collect::<std::result::Result<Vec<_>,_>>()?;
        Ok(items)
    }
    /// Only reserved/active routes on non-revoked devices are shown. Revoked
    /// device rows are retained for auditing but never treated as live sites.
    pub fn list_tenant_routes(&self,tenant:&str,limit:i64,offset:i64)->Result<Vec<Route>>{
        self.ensure_tenant(tenant)?;
        if !(1..=100).contains(&limit) || !(0..=1_000_000).contains(&offset){
            return Err(NetworkError::Invalid);
        }
        let mut stmt=self.db.prepare(
            "SELECT r.id,r.tenant_id,r.agent_id,r.site_id,r.hostname
             FROM network_routes r
             JOIN network_agents a ON a.id=r.agent_id AND a.tenant_id=r.tenant_id
             WHERE r.tenant_id=?1 AND r.state IN ('reserved','active')
               AND a.revoked_at IS NULL
             ORDER BY r.created_at DESC,r.id DESC LIMIT ?2 OFFSET ?3"
        )?;
        let items=stmt.query_map(params![tenant,limit,offset],|row|Ok(Route{
            id:row.get(0)?,tenant_id:row.get(1)?,agent_id:row.get(2)?,
            site_id:row.get(3)?,hostname:row.get(4)?
        }))?.collect::<std::result::Result<Vec<_>,_>>()?;
        Ok(items)
    }
    pub fn create_tenant(&mut self,name:&str)->Result<Tenant>{
        let name=name.trim();
        if name.is_empty() || name.chars().count()>80 {return Err(NetworkError::Invalid);}
        let tenant=Tenant{
            id:Uuid::new_v4().to_string(),name:name.into(),
            max_devices:DEFAULT_DEVICES,max_hostnames:DEFAULT_HOSTNAMES
        };
        let created_at=Utc::now().to_rfc3339();
        let tx=self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO network_tenants(id,name,max_devices,max_hostnames,created_at)
                VALUES(?1,?2,?3,?4,?5)",
            params![tenant.id,tenant.name,tenant.max_devices,tenant.max_hostnames,&created_at]
        )?;
        // A tenant must not exist without its matching audit record.
        tx.execute("INSERT INTO network_audit(tenant_id,ts,action,subject_id)
            VALUES(?1,?2,'tenant_created',?3)",
            params![&tenant.id,&created_at,&tenant.id])?;
        tx.commit()?;
        Ok(tenant)
    }
    /// Call only from authenticated tenant-owner/administrator context.
    pub fn issue_invitation(&mut self, tenant:&str, ttl_seconds:i64)->Result<Invitation>{
        self.ensure_tenant(tenant)?;
        if !(60..=3600).contains(&ttl_seconds){return Err(NetworkError::Invalid);}
        // 2 independent OS-CSPRNG-backed UUIDv4 values provide ~244 bits
        // of unpredictable invitation material.
        let token=format!("{}{}",Uuid::new_v4().simple(),Uuid::new_v4().simple());
        let expires_unix=Utc::now().timestamp()+ttl_seconds;
        let created_at=Utc::now().to_rfc3339();
        let tx=self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("INSERT INTO network_invitations(id,tenant_id,token_hash,expires_unix,created_at)
            VALUES(?1,?2,?3,?4,?5)",
            params![Uuid::new_v4().to_string(),tenant,token_hash(&token),expires_unix,&created_at]
        )?;
        // Audit insertion and invitation issuance succeed or roll back together.
        tx.execute("INSERT INTO network_audit(tenant_id,ts,action,subject_id)
            VALUES(?1,?2,'enrollment_invitation_issued','agent-pending')",
            params![tenant,&created_at])?;
        tx.commit()?;
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
    /// This reserves a globally unique label in a non-routable state.
    /// Real routing requires a separate authenticated proof-of-possession
    /// gateway and explicit provisioning approval, not implemented here.
    /// No DNS or tunnel mutation occurs.
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
            "SELECT COUNT(*) FROM network_routes WHERE tenant_id=?1 AND state IN ('reserved','active')",
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
    /// This canonical challenge message binds the signature to a tenant,
    /// exact device, reserved route and this network's DNS namespace.
    /// No DNS or routing action follows from signing it.
    pub fn challenge_message(&self,tenant:&str,agent:&str,route:&str,challenge:&str)->Result<Vec<u8>>{
        for id in [tenant,agent,route] { require_uuid(id)?; }
        if challenge.len()!=64 || !challenge.bytes().all(|b|b.is_ascii_hexdigit()) {
            return Err(NetworkError::InvalidChallenge);
        }
        Ok(format!("HOSTIFY_DEVICE_ROUTE_PROOF_V1\n{}\n{}\n{}\n{}\n{}\n",
            self.base_domain,tenant,agent,route,challenge).into_bytes())
    }
    /// Administrative action. A device with a valid enrollment token has no
    /// privileges until it can prove possession of its Ed25519 private key.
    pub fn issue_route_challenge(
        &mut self,tenant:&str,agent:&str,route:&str,ttl_seconds:i64,
    )->Result<DeviceChallenge>{
        for id in [tenant,agent,route] {require_uuid(id)?;}
        if !(30..=300).contains(&ttl_seconds){return Err(NetworkError::Invalid);}
        let tx=self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let eligible:i64=tx.query_row(
            "SELECT COUNT(*) FROM network_routes r
                JOIN network_agents a ON a.id=r.agent_id
                JOIN network_tenants t ON t.id=r.tenant_id
                WHERE r.id=?1 AND r.tenant_id=?2 AND r.agent_id=?3
                AND r.state='reserved' AND a.revoked_at IS NULL AND t.state='active'",
            params![route,tenant,agent],|r|r.get(0))?;
        if eligible!=1{return Err(NetworkError::Ownership);}
        let token=format!("{}{}",Uuid::new_v4().simple(),Uuid::new_v4().simple());
        let now=Utc::now().timestamp();
        let expires_unix=now+ttl_seconds;
        tx.execute("INSERT INTO network_route_challenges
            (id,tenant_id,agent_id,route_id,challenge_hash,expires_unix,created_at)
            VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![Uuid::new_v4().to_string(),tenant,agent,route,token_hash(&token),
                expires_unix,Utc::now().to_rfc3339()])?;
        tx.commit()?;
        Ok(DeviceChallenge{value:token,expires_unix})
    }
    /// Cryptographic identity check ONLY. A successful signature stores proof
    /// for an authenticated gateway; it NEVER activates DNS or public routing.
    pub fn verify_route_signature(
        &mut self,tenant:&str,agent:&str,route:&str,challenge:&str,signature_hex:&str,
    )->Result<()>{
        let signed_message=self.challenge_message(tenant,agent,route,challenge)?;
        let signature=hex::decode(signature_hex).map_err(|_|NetworkError::InvalidChallenge)?;
        if signature.len()!=64{return Err(NetworkError::InvalidChallenge);}
        let now=Utc::now().timestamp();
        let tx=self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let entry:Option<(String,String,String,String,i64,Option<String>)>=tx.query_row(
            "SELECT id,tenant_id,agent_id,route_id,expires_unix,consumed_at
                FROM network_route_challenges WHERE challenge_hash=?1",
            [token_hash(challenge)],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,
                r.get(3)?,r.get(4)?,r.get(5)?))).optional()?;
        let (challenge_id,owner,device,target,expires,consumed)=
            entry.ok_or(NetworkError::InvalidChallenge)?;
        if owner!=tenant || device!=agent || target!=route ||
            consumed.is_some() || expires<=now {
            return Err(NetworkError::InvalidChallenge);
        }
        let public_key:Option<String>=tx.query_row(
            "SELECT a.public_key_hex FROM network_agents a
                JOIN network_routes r ON r.agent_id=a.id
                JOIN network_tenants t ON t.id=a.tenant_id
                WHERE a.id=?1 AND a.tenant_id=?2 AND r.id=?3
                AND r.tenant_id=?2 AND r.state='reserved'
                AND a.revoked_at IS NULL AND t.state='active'",
            params![agent,tenant,route],|r|r.get(0)).optional()?;
        let key=public_key.ok_or(NetworkError::Unauthorized)?;
        let raw_key=hex::decode(key).map_err(|_|NetworkError::Invalid)?;
        ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519,&raw_key)
            .verify(&signed_message,&signature).map_err(|_|NetworkError::Unauthorized)?;
        let verified_at=Utc::now().to_rfc3339();
        let affected=tx.execute(
            "UPDATE network_route_challenges SET consumed_at=?1
                WHERE id=?2 AND consumed_at IS NULL AND expires_unix>?3",
            params![&verified_at,&challenge_id,now])?;
        if affected!=1{return Err(NetworkError::InvalidChallenge);}
        let inserted=tx.execute(
            "INSERT OR IGNORE INTO network_agent_proofs(route_id,tenant_id,agent_id,verified_at)
                VALUES(?1,?2,?3,?4)",
            params![route,tenant,agent,&verified_at])?;
        if inserted!=1{return Err(NetworkError::InvalidChallenge);}
        tx.execute(
            "INSERT INTO network_audit(tenant_id,ts,action,subject_id)
                VALUES(?1,?2,'device_identity_verified',?3)",
            params![tenant,&verified_at,route])?;
        tx.commit()?;
        Ok(())
    }
    /// This reads identity proof only; it is *not* a routable/public check.
    /// A revoked device or disabled route invalidates the proof's usefulness.
    pub fn has_verified_identity(&self,tenant:&str,agent:&str,route:&str)->Result<bool>{
        for id in [tenant,agent,route]{require_uuid(id)?;}
        let count:i64=self.db.query_row(
            "SELECT COUNT(*) FROM network_agent_proofs p
                JOIN network_agents a ON a.id=p.agent_id
                JOIN network_routes r ON r.id=p.route_id
                JOIN network_tenants t ON t.id=p.tenant_id
                WHERE p.tenant_id=?1 AND p.agent_id=?2 AND p.route_id=?3
                AND r.tenant_id=?1 AND r.agent_id=?2 AND r.state='reserved'
                AND a.revoked_at IS NULL AND t.state='active'",
            params![tenant,agent,route],|r|r.get(0))?;
        Ok(count==1)
    }

    /// Disabled/revoked agents must never appear as a routable destination.
    /// Only routes explicitly activated by a future mTLS/Ed25519 gateway
    /// may be returned here. Newly enrolled devices remain non-routable.
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
    fn debug_format_never_exposes_enrollment_or_proof_credentials(){
        let enrollment=Invitation{token:"enrollment-only-secret".into(),expires_unix:42};
        let challenge=DeviceChallenge{value:"proof-only-secret".into(),expires_unix:43};
        let invite_text=format!("{enrollment:?}");
        let challenge_text=format!("{challenge:?}");
        assert!(invite_text.contains("[REDACTED]"));
        assert!(challenge_text.contains("[REDACTED]"));
        assert!(invite_text.contains("42"));
        assert!(challenge_text.contains("43"));
        assert!(!invite_text.contains(&enrollment.token));
        assert!(!challenge_text.contains(&challenge.value));
    }
    #[test]
    fn audit_failure_cannot_leave_an_unlogged_tenant_or_device_invitation(){
        let mut registry=Registry::memory("example.test").unwrap();
        registry.db.execute_batch("
            CREATE TRIGGER deny_tenant_audit BEFORE INSERT ON network_audit
            WHEN NEW.action='tenant_created'
            BEGIN SELECT RAISE(FAIL,'audit rejected'); END;
        ").unwrap();
        assert!(registry.create_tenant("Should Roll Back").is_err());
        let tenants:i64=registry.db.query_row(
            "SELECT COUNT(*) FROM network_tenants WHERE name='Should Roll Back'",
            [],|r|r.get(0)).unwrap();
        assert_eq!(tenants,0);
        registry.db.execute_batch("DROP TRIGGER deny_tenant_audit;").unwrap();
        let tenant=registry.create_tenant("Normal Tenant").unwrap();
        registry.db.execute_batch("
            CREATE TRIGGER deny_invite_audit BEFORE INSERT ON network_audit
            WHEN NEW.action='enrollment_invitation_issued'
            BEGIN SELECT RAISE(FAIL,'audit rejected'); END;
        ").unwrap();
        assert!(registry.issue_invitation(&tenant.id,120).is_err());
        let invites:i64=registry.db.query_row(
            "SELECT COUNT(*) FROM network_invitations WHERE tenant_id=?1",
            [&tenant.id],|r|r.get(0)).unwrap();
        assert_eq!(invites,0);
    }
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
        // Enrollment alone is not key possession proof. No route is active.
        assert!(registry.lookup_active_route(&route.hostname).unwrap().is_none());
        assert!(registry.revoke_agent(&b.id,&da.id).is_err());
        registry.revoke_agent(&a.id,&da.id).unwrap();
        assert!(registry.lookup_active_route(&route.hostname).unwrap().is_none());
        assert!(registry.reserve_hostname(&b.id,&db.id,&Uuid::new_v4().to_string(),"alpha-app").is_err());
    }
    #[test]
    fn security_audit_feed_is_tenant_scoped_bounded_and_secret_free(){
        let mut registry=Registry::memory("example.test").unwrap();
        let a=registry.create_tenant("A").unwrap();
        let b=registry.create_tenant("B").unwrap();
        let invitation=registry.issue_invitation(&a.id,300).unwrap();
        let full=registry.list_tenant_audit(&a.id,100,0).unwrap();
        assert_eq!(full.len(),2);
        assert_eq!(full[0].action,"enrollment_invitation_issued");
        assert_eq!(full[1].action,"tenant_created");
        assert_eq!(full[0].tenant_id,a.id);
        assert!(!full.iter().any(|entry|
            entry.subject_id==invitation.token||
            entry.subject_id.contains('@')));
        let first=registry.list_tenant_audit(&a.id,1,0).unwrap();
        let second=registry.list_tenant_audit(&a.id,1,1).unwrap();
        assert_ne!(first[0].id,second[0].id);
        let foreign=registry.list_tenant_audit(&b.id,100,0).unwrap();
        assert_eq!(foreign.len(),1);
        assert_eq!(foreign[0].tenant_id,b.id);
        assert!(registry.list_tenant_audit(&a.id,0,0).is_err());
        assert!(registry.list_tenant_audit(&a.id,101,0).is_err());
        assert!(registry.list_tenant_audit(&a.id,1,-1).is_err());
        assert!(registry.list_tenant_audit("invalid",10,0).is_err());
        registry.db.execute("UPDATE network_tenants SET state='suspended' WHERE id=?1",
            [&a.id]).unwrap();
        assert!(registry.list_tenant_audit(&a.id,10,0).is_err());
    }
    #[test]
    fn tenant_inventory_is_scoped_paginated_and_revocation_safe(){
        let mut r=Registry::memory("example.test").unwrap();
        let a=r.create_tenant("A").unwrap();
        let b=r.create_tenant("B").unwrap();
        let first=r.issue_invitation(&a.id,300).unwrap();
        let second=r.issue_invitation(&b.id,300).unwrap();
        let agent_a=r.enroll_agent(&first.token,&key(80)).unwrap();
        let agent_b=r.enroll_agent(&second.token,&key(81)).unwrap();
        let route_a=r.reserve_hostname(&a.id,&agent_a.id,&Uuid::new_v4().to_string(),"home-a").unwrap();
        let route_b=r.reserve_hostname(&b.id,&agent_b.id,&Uuid::new_v4().to_string(),"home-b").unwrap();
        let aa=r.list_tenant_agents(&a.id,1,0).unwrap();
        assert_eq!(aa.len(),1);
        assert_eq!(aa[0].id,agent_a.id);
        assert!(r.list_tenant_agents(&a.id,1,1).unwrap().is_empty());
        let ar=r.list_tenant_routes(&a.id,100,0).unwrap();
        assert_eq!(ar.len(),1);
        assert_eq!(ar[0].id,route_a.id);
        assert_ne!(ar[0].hostname,route_b.hostname);
        assert!(r.list_tenant_agents(&a.id,101,0).is_err());
        assert!(r.list_tenant_routes(&a.id,1,-1).is_err());
        assert!(r.list_tenant_routes("not-a-tenant",1,0).is_err());
        r.revoke_agent(&a.id,&agent_a.id).unwrap();
        assert!(r.list_tenant_agents(&a.id,1,0).unwrap().is_empty());
        assert!(r.list_tenant_routes(&a.id,1,0).unwrap().is_empty());
        assert_eq!(r.list_tenant_agents(&b.id,1,0).unwrap()[0].id,agent_b.id);
    }
    #[test]
    fn reserved_names_and_invalid_labels_are_rejected(){
        for name in ["api","admin","www","mail","root","xn--test","a","-foo","foo.","some.name","bad_name"] {
            assert!(normalize_label(name).is_err(),"{name}");
        }
        assert_eq!(normalize_label(" My-Site ").unwrap(),"my-site");
    }
    #[test]
    fn ed25519_identity_proof_is_bound_to_tenant_agent_route_and_expiry(){
        use ring::{rand::SystemRandom,signature::{Ed25519KeyPair,KeyPair}};
        let rng=SystemRandom::new();
        let private=Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        let keypair=Ed25519KeyPair::from_pkcs8(private.as_ref()).unwrap();
        let mut registry=Registry::memory("example.test").unwrap();
        let tenant=registry.create_tenant("Authorized tenant").unwrap();
        let outsider=registry.create_tenant("Other tenant").unwrap();
        let invitation=registry.issue_invitation(&tenant.id,300).unwrap();
        let agent=registry.enroll_agent(&invitation.token,&hex::encode(keypair.public_key().as_ref())).unwrap();
        let route=registry.reserve_hostname(&tenant.id,&agent.id,&Uuid::new_v4().to_string(),"verified-device").unwrap();
        assert!(!registry.has_verified_identity(&tenant.id,&agent.id,&route.id).unwrap());
        assert!(registry.issue_route_challenge(&outsider.id,&agent.id,&route.id,60).is_err());
        let challenge=registry.issue_route_challenge(&tenant.id,&agent.id,&route.id,60).unwrap();
        let stored_hash:String=registry.db.query_row(
            "SELECT challenge_hash FROM network_route_challenges",[],|r|r.get(0)).unwrap();
        assert_eq!(stored_hash,token_hash(&challenge.value));
        assert_ne!(stored_hash,challenge.value);
        let bytes=registry.challenge_message(&tenant.id,&agent.id,&route.id,&challenge.value).unwrap();
        let signed=hex::encode(keypair.sign(&bytes).as_ref());
        let mut forged=signed.clone();
        // Ensure a different signature byte even if the original starts 00.
        let replacement=if &signed[..2]=="00" {"ff"} else {"00"};
        forged.replace_range(0..2,replacement);
        assert!(registry.verify_route_signature(&tenant.id,&agent.id,&route.id,&challenge.value,&forged).is_err());
        assert!(!registry.has_verified_identity(&tenant.id,&agent.id,&route.id).unwrap());
        registry.verify_route_signature(&tenant.id,&agent.id,&route.id,&challenge.value,&signed).unwrap();
        assert!(registry.has_verified_identity(&tenant.id,&agent.id,&route.id).unwrap());
        assert!(registry.verify_route_signature(&tenant.id,&agent.id,&route.id,&challenge.value,&signed).is_err());
        // Signing is not consent for DNS mutation, tunnel creation, or public routing.
        assert!(registry.lookup_active_route(&route.hostname).unwrap().is_none());
        registry.revoke_agent(&tenant.id,&agent.id).unwrap();
        assert!(!registry.has_verified_identity(&tenant.id,&agent.id,&route.id).unwrap());
    }
    #[test]
    fn challenge_cannot_be_reused_after_expiry(){
        use ring::{rand::SystemRandom,signature::{Ed25519KeyPair,KeyPair}};
        let rng=SystemRandom::new();
        let private=Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        let pair=Ed25519KeyPair::from_pkcs8(private.as_ref()).unwrap();
        let mut registry=Registry::memory("example.test").unwrap();
        let tenant=registry.create_tenant("T").unwrap();
        let invitation=registry.issue_invitation(&tenant.id,60).unwrap();
        let agent=registry.enroll_agent(&invitation.token,&hex::encode(pair.public_key().as_ref())).unwrap();
        let route=registry.reserve_hostname(&tenant.id,&agent.id,&Uuid::new_v4().to_string(),"expired-proof").unwrap();
        let challenge=registry.issue_route_challenge(&tenant.id,&agent.id,&route.id,30).unwrap();
        registry.db.execute("UPDATE network_route_challenges SET expires_unix=0
            WHERE challenge_hash=?1",[token_hash(&challenge.value)]).unwrap();
        let message=registry.challenge_message(&tenant.id,&agent.id,&route.id,&challenge.value).unwrap();
        assert!(registry.verify_route_signature(&tenant.id,&agent.id,&route.id,&challenge.value,
            &hex::encode(pair.sign(&message).as_ref())).is_err());
        assert!(!registry.has_verified_identity(&tenant.id,&agent.id,&route.id).unwrap());
    }
    #[test]
    fn network_namespace_is_pinned_even_without_existing_routes(){
        let tmp=tempfile::tempdir().unwrap();
        let path=tmp.path().join("network.db");
        drop(Registry::open(&path,"first.example.test").unwrap());
        assert!(matches!(Registry::open(&path,"second.example.test"),Err(NetworkError::Invalid)));
        assert!(Registry::open(&path,"FIRST.EXAMPLE.TEST").is_ok());
    }
    #[test]
    fn legacy_namespace_migration_rejects_other_domain_and_preserves_routes(){
        let tmp=tempfile::tempdir().unwrap();
        let path=tmp.path().join("legacy.db");
        {
            let mut registry=Registry::open(&path,"example.test").unwrap();
            let tenant=registry.create_tenant("Existing").unwrap();
            let invitation=registry.issue_invitation(&tenant.id,120).unwrap();
            let agent=registry.enroll_agent(&invitation.token,&key(15)).unwrap();
            registry.reserve_hostname(&tenant.id,&agent.id,&Uuid::new_v4().to_string(),"existing-site").unwrap();
            // Simulate a pre-config-metadata database.
            registry.db.execute("DELETE FROM network_config WHERE key='base_domain'",[]).unwrap();
        }
        assert!(matches!(Registry::open(&path,"wrong.test"),Err(NetworkError::Invalid)));
        let registry=Registry::open(&path,"example.test").unwrap();
        let stored:String=registry.db.query_row(
            "SELECT value FROM network_config WHERE key='base_domain'",
            [],|row|row.get(0)).unwrap();
        assert_eq!(stored,"example.test");
        let count:i64=registry.db.query_row(
            "SELECT COUNT(*) FROM network_routes WHERE hostname='existing-site.example.test'",
            [],|row|row.get(0)).unwrap();
        assert_eq!(count,1);
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
        let reserved_count:i64=r.db.query_row(
            "SELECT COUNT(*) FROM network_routes WHERE hostname=?1 AND state='reserved'",
            [&hostname],|row|row.get(0)).unwrap();
        assert_eq!(reserved_count,1);
        assert!(r.lookup_active_route(&hostname).unwrap().is_none());
        r.revoke_agent(&tenant,&agent).unwrap();
        assert!(r.lookup_active_route(&hostname).unwrap().is_none());
    }
}
