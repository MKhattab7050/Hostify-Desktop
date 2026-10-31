//! Central-only account and session registry. This module never creates
//! HTTP listeners or Tauri commands, and must be called behind authenticated
//! control-plane APIs with TLS and per-IP abuse protection.
use chrono::Utc;
use ring::{pbkdf2,rand::{SecureRandom,SystemRandom}};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Serialize,Deserialize};
use sha2::{Sha256,Digest};
use std::{num::NonZeroU32,path::Path,time::Duration};
use thiserror::Error;
use uuid::Uuid;

const PASSWORD_ITERATIONS:u32=210_000;
const SESSION_TTL:i64=12*3600;
const MAX_FAILURES:i64=5;
const LOCK_SECONDS:i64=15*60;

#[derive(Debug,Error)]
pub enum AccountError {
    #[error("invalid account data")]
    Invalid,
    #[error("account or authentication token rejected")]
    Authentication,
    #[error("account temporarily locked after unsuccessful login attempts")]
    Locked,
    #[error("insufficient tenant permissions")]
    Forbidden,
    #[error("invitation expired, revoked, already consumed or unavailable")]
    Invitation,
    #[error("tenant or user unavailable")]
    Unavailable,
    #[error("cryptographic random generator failed")]
    Random,
    #[error("database error: {0}")]
    Database(#[from]rusqlite::Error),
}
pub type Result<T>=std::result::Result<T,AccountError>;
#[derive(Debug,Clone,Copy,PartialEq,Eq,Serialize,Deserialize)]
#[serde(rename_all="lowercase")]
pub enum Role {Owner,Admin,Viewer}
impl Role {
    fn db(self)->&'static str{
        match self{Self::Owner=>"owner",Self::Admin=>"admin",Self::Viewer=>"viewer"}
    }
    fn parse(input:&str)->Result<Self>{
        match input {
            "owner"=>Ok(Self::Owner),"admin"=>Ok(Self::Admin),
            "viewer"=>Ok(Self::Viewer),_=>Err(AccountError::Authentication)
        }
    }
}
#[derive(Debug,Clone,Copy)]
pub enum Capability {View, ManageSites, ManageDevices, ManageUsers, ManageBilling}
pub fn allowed(role:Role,action:Capability)->bool{
    match role {
        Role::Owner=>true,
        Role::Admin=>!matches!(action,Capability::ManageBilling),
        Role::Viewer=>matches!(action,Capability::View)
    }
}
#[derive(Debug,Clone,Serialize)]
#[serde(rename_all="camelCase")]
pub struct Principal {
    pub user_id:String,
    pub tenant_id:String,
    pub email:String,
    pub role:Role,
}
/// Public member inventory intentionally excludes password hashes, session tokens
/// and password lockout counters.
#[derive(Debug,Clone,Serialize)]
#[serde(rename_all="camelCase")]
pub struct TenantMember {
    pub user_id:String,
    pub tenant_id:String,
    pub email:String,
    pub role:Role,
    pub status:String,
    pub created_at:String,
}
/// A user's own active session inventory. No bearer token or digest is returned.
#[derive(Debug,Clone,Serialize)]
#[serde(rename_all="camelCase")]
pub struct ActiveSession {
    pub session_id:String,
    pub created_at:String,
    pub expires_unix:i64,
    pub is_current:bool,
}
/// A token returned exactly once. Only the SHA-256 digest is persisted.
pub struct Session {
    pub token:String,
    pub expires_unix:i64,
    pub principal:Principal,
}
/// Bearer credentials must never appear in diagnostic logs.
impl std::fmt::Debug for Session {
    fn fmt(&self, f:&mut std::fmt::Formatter<'_>)->std::fmt::Result {
        f.debug_struct("Session")
            .field("token",&"[REDACTED]")
            .field("expires_unix",&self.expires_unix)
            .field("user_id",&self.principal.user_id)
            .field("tenant_id",&self.principal.tenant_id)
            .field("role",&self.principal.role)
            .finish()
    }
}
pub struct MemberInvitation {
    pub token:String,
    pub expires_unix:i64,
}
impl std::fmt::Debug for MemberInvitation {
    fn fmt(&self, f:&mut std::fmt::Formatter<'_>)->std::fmt::Result {
        f.debug_struct("MemberInvitation")
            .field("token",&"[REDACTED]")
            .field("expires_unix",&self.expires_unix)
            .finish()
    }
}
pub(crate) fn canonical_email(raw:&str)->Result<String>{
    let s=raw.trim().to_ascii_lowercase();
    if s.len()<5||s.len()>254||s.matches('@').count()!=1||
        s.bytes().any(|b|b.is_ascii_whitespace()||b.is_ascii_control()) || !s.is_ascii(){
        return Err(AccountError::Invalid);
    }
    let (local,domain)=s.split_once('@').ok_or(AccountError::Invalid)?;
    if local.is_empty()||domain.len()<3||!domain.contains('.')||
        domain.starts_with('.')||domain.ends_with('.')||
        !domain.bytes().all(|b|b.is_ascii_lowercase()||b.is_ascii_digit()||
            matches!(b,b'-'|b'.')){return Err(AccountError::Invalid);}
    Ok(s)
}
pub(crate) fn validate_password(password:&str)->Result<()>{
    if !(12..=256).contains(&password.len()) || password.chars().any(|c|c.is_control()){
        return Err(AccountError::Invalid);
    }
    Ok(())
}
fn random_hex(count:usize)->Result<String>{
    let mut bytes=vec![0;count];
    SystemRandom::new().fill(&mut bytes).map_err(|_|AccountError::Random)?;
    Ok(hex::encode(bytes))
}
fn digest(s:&str)->String{format!("{:x}",Sha256::digest(s.as_bytes()))}
fn key_from_password(password:&str,salt:&[u8])->[u8;32]{
    let mut derived=[0u8;32];
    pbkdf2::derive(pbkdf2::PBKDF2_HMAC_SHA256,
        NonZeroU32::new(PASSWORD_ITERATIONS).unwrap(),salt,password.as_bytes(),&mut derived);
    derived
}
fn hash_password(password:&str)->Result<(String,String)>{
    validate_password(password)?;
    let salt=random_hex(16)?;
    let bytes=hex::decode(&salt).map_err(|_|AccountError::Random)?;
    Ok((salt,hex::encode(key_from_password(password,&bytes))))
}
fn correct_password(password:&str,salt:&str,expected:&str)->bool{
    let Ok(salt)=hex::decode(salt)else{return false};
    let Ok(expected)=hex::decode(expected)else{return false};
    if expected.len()!=32||salt.len()!=16{return false;}
    pbkdf2::verify(pbkdf2::PBKDF2_HMAC_SHA256,
        NonZeroU32::new(PASSWORD_ITERATIONS).unwrap(),
        &salt,password.as_bytes(),&expected).is_ok()
}
fn require_uuid(id:&str)->Result<()>{
    Uuid::parse_str(id).map(|_|()).map_err(|_|AccountError::Invalid)
}

pub struct Accounts{db:Connection}
impl Accounts{
    /// Open after the central network_tenants registry has been initialized.
    pub fn open(path:&Path)->Result<Self>{
        let db=Connection::open(path)?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.execute_batch("
            PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS network_users(
                id TEXT PRIMARY KEY,
                tenant_id TEXT NOT NULL REFERENCES network_tenants(id),
                email TEXT NOT NULL,
                salt TEXT NOT NULL,
                password_hash TEXT NOT NULL,
                role TEXT NOT NULL CHECK(role IN ('owner','admin','viewer')),
                status TEXT NOT NULL DEFAULT 'active' CHECK(status IN ('active','disabled')),
                failed_attempts INTEGER NOT NULL DEFAULT 0,
                locked_until_unix INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                UNIQUE(tenant_id,email)
            );
            CREATE TABLE IF NOT EXISTS network_member_invitations(
                id TEXT PRIMARY KEY,
                tenant_id TEXT NOT NULL REFERENCES network_tenants(id),
                email TEXT NOT NULL,
                role TEXT NOT NULL CHECK(role IN ('admin','viewer')),
                inviter_id TEXT NOT NULL REFERENCES network_users(id),
                token_hash TEXT NOT NULL UNIQUE,
                expires_unix INTEGER NOT NULL,
                consumed_at TEXT,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS network_sessions(
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL REFERENCES network_users(id),
                tenant_id TEXT NOT NULL REFERENCES network_tenants(id),
                token_hash TEXT NOT NULL UNIQUE,
                expires_unix INTEGER NOT NULL,
                revoked_at TEXT,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_network_sessions_tenant_user
                ON network_sessions(tenant_id,user_id,expires_unix);
        ")?;
        Ok(Self{db})
    }
    fn active_tenant(&self,tenant:&str)->Result<()>{
        require_uuid(tenant)?;
        let state:Option<String>=self.db.query_row(
            "SELECT state FROM network_tenants WHERE id=?1",[tenant],|r|r.get(0)).optional()?;
        if state.as_deref()!=Some("active"){return Err(AccountError::Unavailable);}
        Ok(())
    }
    /// Trusted operator provisioning only. New tenant, first owner and audit
    /// event commit in one SQLite transaction or none of them are persisted.
    /// This is not a self-signup API and must never be exposed publicly.
    pub fn provision_owner_tenant(
        &mut self,name:&str,email:&str,password:&str,
    )->Result<(crate::network::Tenant,Principal)>{
        use crate::network::{Tenant,DEFAULT_DEVICES,DEFAULT_HOSTNAMES};
        let name=name.trim();
        if name.is_empty() || name.chars().count()>80 {
            return Err(AccountError::Invalid);
        }
        let email=canonical_email(email)?;
        let (salt,password_hash)=hash_password(password)?;
        let tenant=Tenant{
            id:Uuid::new_v4().to_string(),name:name.to_owned(),
            max_devices:DEFAULT_DEVICES,max_hostnames:DEFAULT_HOSTNAMES,
        };
        let user_id=Uuid::new_v4().to_string();
        let created_at=Utc::now().to_rfc3339();
        let tx=self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO network_tenants(id,name,max_devices,max_hostnames,created_at)
                VALUES(?1,?2,?3,?4,?5)",
            params![&tenant.id,&tenant.name,tenant.max_devices,tenant.max_hostnames,&created_at]
        )?;
        tx.execute(
            "INSERT INTO network_users(id,tenant_id,email,salt,password_hash,role,created_at)
                VALUES(?1,?2,?3,?4,?5,'owner',?6)",
            params![&user_id,&tenant.id,&email,salt,password_hash,&created_at]
        )?;
        tx.execute(
            "INSERT INTO network_audit(tenant_id,ts,action,subject_id)
                VALUES(?1,?2,'tenant_created',?3)",
            params![&tenant.id,&created_at,&tenant.id]
        )?;
        tx.commit()?;
        let owner=Principal{user_id,tenant_id:tenant.id.clone(),email,role:Role::Owner};
        Ok((tenant,owner))
    }
    /// OFFLINE operator/bootstrap only. Do not expose over a public API.
    /// Rejects a second owner rather than allowing account takeover.
    pub fn bootstrap_owner(&mut self,tenant:&str,email:&str,password:&str)->Result<Principal>{
        self.active_tenant(tenant)?;
        let email=canonical_email(email)?;
        let (salt,password_hash)=hash_password(password)?;
        let tx=self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let count:i64=tx.query_row(
            "SELECT COUNT(*) FROM network_users WHERE tenant_id=?1 AND role='owner'",
            [tenant],|r|r.get(0))?;
        if count>0{return Err(AccountError::Forbidden);}
        let id=Uuid::new_v4().to_string();
        tx.execute("INSERT INTO network_users(id,tenant_id,email,salt,password_hash,role,created_at)
            VALUES(?1,?2,?3,?4,?5,'owner',?6)",
            params![&id,tenant,&email,salt,password_hash,Utc::now().to_rfc3339()])?;
        tx.commit()?;
        Ok(Principal{user_id:id,tenant_id:tenant.into(),email,role:Role::Owner})
    }
    /// Fail closed after 5 failures; remote HTTP gateway must add IP rate
    /// limits and response padding, not just this per-account limiter.
    pub fn sign_in(&mut self,tenant:&str,email:&str,password:&str)->Result<Session>{
        self.active_tenant(tenant)?;
        let email=canonical_email(email)?;
        let user:Option<(String,String,String,String,String,i64,i64)>=self.db.query_row(
            "SELECT id,salt,password_hash,role,status,failed_attempts,locked_until_unix
                FROM network_users WHERE tenant_id=?1 AND email=?2",
            params![tenant,&email],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,
                r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).optional()?;
        let Some((id,salt,stored,role,status,failures,locked))=user else{
            // Same expensive KDF work for an unknown email to reduce enumeration.
            let _=correct_password(password,"00000000000000000000000000000000",
                "0000000000000000000000000000000000000000000000000000000000000000");
            return Err(AccountError::Authentication);
        };
        let now=Utc::now().timestamp();
        if status!="active"{return Err(AccountError::Authentication);}
        if locked>now{return Err(AccountError::Locked);}
        if !correct_password(password,&salt,&stored){
            // An expired lockout starts a fresh failure window. Otherwise a
            // single wrong password after 15 minutes would immediately lock
            // the account again because failed_attempts remained at five.
            let next=if locked>0 && locked<=now {1} else {failures.saturating_add(1)};
            let until=if next>=MAX_FAILURES {now+LOCK_SECONDS}else{0};
            self.db.execute("UPDATE network_users SET failed_attempts=?1,locked_until_unix=?2
                WHERE id=?3",params![next,until,id])?;
            return Err(AccountError::Authentication);
        }
        let principal=Principal{
            user_id:id.clone(),tenant_id:tenant.into(),email,role:Role::parse(&role)?
        };
        let token=random_hex(32)?;
        let expires_unix=now+SESSION_TTL;
        let tx=self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("UPDATE network_users SET failed_attempts=0,locked_until_unix=0 WHERE id=?1",
            [&id])?;
        tx.execute("INSERT INTO network_sessions(id,user_id,tenant_id,token_hash,expires_unix,created_at)
            VALUES(?1,?2,?3,?4,?5,?6)",
            params![Uuid::new_v4().to_string(),&id,tenant,digest(&token),expires_unix,
                Utc::now().to_rfc3339()])?;
        tx.commit()?;
        Ok(Session{token,expires_unix,principal})
    }
    pub fn authorize(&self,token:&str,tenant:&str,cap:Capability)->Result<Principal>{
        require_uuid(tenant)?;
        if token.len()!=64||!token.bytes().all(|b|b.is_ascii_hexdigit()){
            return Err(AccountError::Authentication);
        }
        let result:Option<(String,String,String,String)>=self.db.query_row(
            "SELECT u.id,u.tenant_id,u.email,u.role
                FROM network_sessions s JOIN network_users u ON u.id=s.user_id
                JOIN network_tenants t ON t.id=u.tenant_id
                WHERE s.token_hash=?1 AND s.tenant_id=?2
                AND s.revoked_at IS NULL AND s.expires_unix>?3
                AND u.status='active' AND t.state='active'
                AND u.tenant_id=s.tenant_id",
            params![digest(token),tenant,Utc::now().timestamp()],
            |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
        let (user_id,tenant_id,email,role)=result.ok_or(AccountError::Authentication)?;
        let role=Role::parse(&role)?;
        if !allowed(role,cap){return Err(AccountError::Forbidden);}
        Ok(Principal{user_id,tenant_id,email,role})
    }
    pub fn sign_out(&mut self,token:&str)->Result<()>{
        if token.len()!=64||!token.bytes().all(|b|b.is_ascii_hexdigit()){
            return Err(AccountError::Authentication);
        }
        self.db.execute("UPDATE network_sessions SET revoked_at=?1
            WHERE token_hash=?2 AND revoked_at IS NULL",
            params![Utc::now().to_rfc3339(),digest(token)])?;
        Ok(())
    }
    /// A user can inspect only their own active sessions. Session token hashes
    /// are read solely to mark the current session and never serialized.
    pub fn list_sessions(&self,token:&str,tenant:&str,limit:i64,offset:i64)->Result<Vec<ActiveSession>>{
        let actor=self.authorize(token,tenant,Capability::View)?;
        if !(1..=100).contains(&limit)||!(0..=100_000).contains(&offset){
            return Err(AccountError::Invalid);
        }
        let current_hash=digest(token);
        let mut stmt=self.db.prepare(
            "SELECT id,created_at,expires_unix,token_hash FROM network_sessions
             WHERE tenant_id=?1 AND user_id=?2
             AND revoked_at IS NULL AND expires_unix>?3
             ORDER BY created_at DESC,id DESC LIMIT ?4 OFFSET ?5")?;
        let rows=stmt.query_map(
            params![tenant,&actor.user_id,Utc::now().timestamp(),limit,offset],
            |r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,
                r.get::<_,i64>(2)?,r.get::<_,String>(3)?)))?;
        let mut items=Vec::new();
        for row in rows{
            let (session_id,created_at,expires_unix,hash)=row?;
            items.push(ActiveSession{
                session_id,created_at,expires_unix,is_current:hash==current_hash,
            });
        }
        Ok(items)
    }
    /// A member may revoke any of their own active sessions, including the
    /// current one. Revocation and audit event must commit together.
    pub fn revoke_session(&mut self,token:&str,tenant:&str,session_id:&str)->Result<()>{
        let actor=self.authorize(token,tenant,Capability::View)?;
        require_uuid(session_id)?;
        let tx=self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let active:i64=tx.query_row(
            "SELECT COUNT(*) FROM network_sessions
             WHERE user_id=?1 AND tenant_id=?2 AND token_hash=?3
               AND revoked_at IS NULL AND expires_unix>?4",
            params![&actor.user_id,tenant,digest(token),Utc::now().timestamp()],
            |r|r.get(0))?;
        if active!=1{return Err(AccountError::Authentication);}
        let now=Utc::now().to_rfc3339();
        let changed=tx.execute(
            "UPDATE network_sessions SET revoked_at=?1
             WHERE id=?2 AND user_id=?3 AND tenant_id=?4
               AND revoked_at IS NULL AND expires_unix>?5",
            params![&now,session_id,&actor.user_id,tenant,Utc::now().timestamp()])?;
        if changed!=1{return Err(AccountError::Unavailable);}
        tx.execute(
            "INSERT INTO network_audit(tenant_id,ts,action,subject_id)
             VALUES(?1,?2,'session_revoked',?3)",
            params![tenant,&now,session_id])?;
        tx.commit()?;
        Ok(())
    }
    /// Change the current member's password only after checking the existing
    /// credential. Revoke every previous session and audit the action atomically.
    /// The caller must sign in again; no password or token enters the audit log.
    pub fn change_password(&mut self,token:&str,tenant:&str,
        current_password:&str,new_password:&str)->Result<()>{
        let principal=self.authorize(token,tenant,Capability::View)?;
        validate_password(new_password)?;
        let (new_salt,new_hash)=hash_password(new_password)?;
        let tx=self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Recheck the active session inside the write transaction so a
        // concurrent administrator revocation cannot authorize this change.
        let old:Option<(String,String)>=tx.query_row(
            "SELECT u.salt,u.password_hash FROM network_users u
             JOIN network_sessions s ON s.user_id=u.id AND s.tenant_id=u.tenant_id
             JOIN network_tenants t ON t.id=u.tenant_id
             WHERE u.id=?1 AND u.tenant_id=?2 AND s.token_hash=?3
               AND s.revoked_at IS NULL AND s.expires_unix>?4
               AND u.status='active' AND t.state='active'",
            params![&principal.user_id,tenant,digest(token),Utc::now().timestamp()],
            |r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        let (old_salt,old_hash)=old.ok_or(AccountError::Authentication)?;
        if !correct_password(current_password,&old_salt,&old_hash) {
            return Err(AccountError::Authentication);
        }
        if correct_password(new_password,&old_salt,&old_hash) {
            return Err(AccountError::Invalid);
        }
        let now=Utc::now().to_rfc3339();
        let changed=tx.execute(
            "UPDATE network_users SET salt=?1,password_hash=?2,
                failed_attempts=0,locked_until_unix=0
             WHERE id=?3 AND tenant_id=?4 AND status='active'",
            params![new_salt,new_hash,&principal.user_id,tenant])?;
        if changed!=1{return Err(AccountError::Authentication);}
        tx.execute(
            "UPDATE network_sessions SET revoked_at=?1
             WHERE tenant_id=?2 AND user_id=?3 AND revoked_at IS NULL",
            params![&now,tenant,&principal.user_id])?;
        tx.execute(
            "INSERT INTO network_audit(tenant_id,ts,action,subject_id)
             VALUES(?1,?2,'password_changed',?3)",
            params![tenant,&now,&principal.user_id])?;
        tx.commit()?;
        Ok(())
    }
    /// List members of exactly one tenant. A viewer cannot enumerate accounts,
    /// including disabled ones. Credentials and failure counters never leave
    /// the account store through this result.
    pub fn list_members(&self,token:&str,tenant:&str,limit:i64,offset:i64)->Result<Vec<TenantMember>>{
        self.authorize(token,tenant,Capability::ManageUsers)?;
        if !(1..=100).contains(&limit) || !(0..=100_000).contains(&offset){
            return Err(AccountError::Invalid);
        }
        let mut stmt=self.db.prepare(
            "SELECT id,tenant_id,email,role,status,created_at FROM network_users
             WHERE tenant_id=?1 ORDER BY created_at DESC,id DESC LIMIT ?2 OFFSET ?3")?;
        let rows=stmt.query_map(params![tenant,limit,offset],|r|{
            Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,
                r.get::<_,String>(2)?,r.get::<_,String>(3)?,
                r.get::<_,String>(4)?,r.get::<_,String>(5)?))
        })?;
        let mut items=Vec::new();
        for row in rows {
            let (user_id,tenant_id,email,role,status,created_at)=row?;
            items.push(TenantMember{
                user_id,tenant_id,email,role:Role::parse(&role)?,status,created_at,
            });
        }
        Ok(items)
    }
    /// Disable a tenant member, revoke existing sessions and record a
    /// non-secret audit event as one SQLite transaction. The final owner
    /// cannot be disabled through this API. Admins may disable viewers only.
    pub fn disable_member(&mut self,token:&str,tenant:&str,user_id:&str)->Result<()>{
        let actor=self.authorize(token,tenant,Capability::ManageUsers)?;
        require_uuid(user_id)?;
        if actor.user_id==user_id {return Err(AccountError::Forbidden);}
        let tx=self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let target:Option<(String,String)>=tx.query_row(
            "SELECT role,status FROM network_users WHERE tenant_id=?1 AND id=?2",
            params![tenant,user_id],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
        let (target_role,status)=target.ok_or(AccountError::Unavailable)?;
        if target_role=="owner" ||
            (actor.role==Role::Admin && target_role!="viewer") {
            return Err(AccountError::Forbidden);
        }
        if status=="disabled" {return Ok(());}
        let now=Utc::now().to_rfc3339();
        let updated=tx.execute(
            "UPDATE network_users SET status='disabled'
                WHERE id=?1 AND tenant_id=?2 AND status='active'",
            params![user_id,tenant])?;
        if updated!=1 {return Err(AccountError::Unavailable);}
        tx.execute(
            "UPDATE network_sessions SET revoked_at=?1
                WHERE user_id=?2 AND tenant_id=?3 AND revoked_at IS NULL",
            params![&now,user_id,tenant])?;
        tx.execute(
            "INSERT INTO network_audit(tenant_id,ts,action,subject_id)
                VALUES(?1,?2,'member_disabled',?3)",
            params![tenant,&now,user_id])?;
        tx.commit()?;
        Ok(())
    }
    pub fn invite_member(&mut self,token:&str,tenant:&str,email:&str,
        role:Role,ttl_seconds:i64)->Result<MemberInvitation>{
        let inviter=self.authorize(token,tenant,Capability::ManageUsers)?;
        if !(60..=24*3600).contains(&ttl_seconds) || role==Role::Owner ||
            (inviter.role==Role::Admin && role!=Role::Viewer) {
            return Err(AccountError::Forbidden);
        }
        let email=canonical_email(email)?;
        let pending_token=random_hex(32)?;
        let expires_unix=Utc::now().timestamp()+ttl_seconds;
        let invitation_id=Uuid::new_v4().to_string();
        let issued_at=Utc::now().to_rfc3339();
        let tx=self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("INSERT INTO network_member_invitations
            (id,tenant_id,email,role,inviter_id,token_hash,expires_unix,created_at)
            VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![&invitation_id,tenant,&email,role.db(),
                &inviter.user_id,digest(&pending_token),expires_unix,&issued_at])?;
        // Audit references only the invitation UUID, never its token or email.
        // Failed audit insertion must not leave an untracked valid invite.
        tx.execute("INSERT INTO network_audit(tenant_id,ts,action,subject_id)
            VALUES(?1,?2,'member_invited',?3)",
            params![tenant,&issued_at,&invitation_id])?;
        tx.commit()?;
        Ok(MemberInvitation{token:pending_token,expires_unix})
    }
    pub fn accept_invitation(&mut self,token:&str,password:&str)->Result<Principal>{
        if token.len()!=64||!token.bytes().all(|b|b.is_ascii_hexdigit()){
            return Err(AccountError::Invitation);
        }
        let (salt,hash)=hash_password(password)?;
        let now=Utc::now().timestamp();
        let tx=self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let info:Option<(String,String,String,String,String,i64,Option<String>)>=tx.query_row(
            "SELECT i.id,i.tenant_id,i.email,i.role,t.state,
                i.expires_unix,i.consumed_at
                FROM network_member_invitations i
                JOIN network_tenants t ON t.id=i.tenant_id
                WHERE i.token_hash=?1",[digest(token)],
            |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,
                r.get(4)?,r.get(5)?,r.get(6)?))).optional()?;
        let (inv,tenant,email,role,state,expiry,consumed)=info.ok_or(AccountError::Invitation)?;
        if state!="active"||expiry<=now||consumed.is_some(){
            return Err(AccountError::Invitation);
        }
        let id=Uuid::new_v4().to_string();
        let now_s=Utc::now().to_rfc3339();
        let changed=tx.execute("UPDATE network_member_invitations SET consumed_at=?1
            WHERE id=?2 AND consumed_at IS NULL AND expires_unix>?3",
            params![&now_s,inv,now])?;
        if changed!=1{return Err(AccountError::Invitation);}
        tx.execute("INSERT INTO network_users(id,tenant_id,email,salt,password_hash,role,created_at)
            VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![&id,&tenant,&email,salt,hash,role,&now_s])?;
        // Consuming an invitation, creating the user and recording the audit
        // must succeed or roll back together. No credential values are logged.
        tx.execute("INSERT INTO network_audit(tenant_id,ts,action,subject_id)
            VALUES(?1,?2,'member_joined',?3)",
            params![&tenant,&now_s,&id])?;
        tx.commit()?;
        Ok(Principal{user_id:id,tenant_id:tenant,email,role:Role::parse(&role)?})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::Registry;
    fn init()->(tempfile::TempDir,String,String,Accounts){
        let tmp=tempfile::tempdir().unwrap();
        let path=tmp.path().join("control-plane.db");
        let mut registry=Registry::open(&path,"example.test").unwrap();
        let one=registry.create_tenant("A").unwrap();
        let two=registry.create_tenant("B").unwrap();
        let accounts=Accounts::open(&path).unwrap();
        (tmp,one.id,two.id,accounts)
    }
    #[test]
    fn atomic_owner_provision_rolls_back_tenant_when_owner_insert_fails(){
        let (dir,_a,_b,mut accounts)=init();
        accounts.db.execute_batch("
            CREATE TRIGGER refuse_owner_insert BEFORE INSERT ON network_users
            WHEN NEW.role='owner'
            BEGIN SELECT RAISE(FAIL,'test owner insert failure'); END;
        ").unwrap();
        let result=accounts.provision_owner_tenant("Orphan Must Not Persist",
            "owner@site.test","long-valid-owner-password");
        assert!(result.is_err());
        let db=Connection::open(dir.path().join("control-plane.db")).unwrap();
        let count:i64=db.query_row(
            "SELECT COUNT(*) FROM network_tenants WHERE name='Orphan Must Not Persist'",
            [],|row|row.get(0)).unwrap();
        assert_eq!(count,0);
        let audit:i64=db.query_row(
            "SELECT COUNT(*) FROM network_audit WHERE action='tenant_created'",
            [],|row|row.get(0)).unwrap();
        // The two pre-existing fixture tenants still have their own audit rows.
        assert_eq!(audit,2);
        accounts.db.execute_batch("DROP TRIGGER refuse_owner_insert").unwrap();
        let (tenant,owner)=accounts.provision_owner_tenant(
            "Valid Tenant","owner@site.test","long-valid-owner-password").unwrap();
        assert_eq!(tenant.id,owner.tenant_id);
        assert_eq!(owner.role,Role::Owner);
        assert!(accounts.sign_in(&tenant.id,"owner@site.test","long-valid-owner-password").is_ok());
    }
    #[test]
    fn invalid_operator_provision_does_not_persist_tenant(){
        let (_dir,_a,_b,mut accounts)=init();
        assert!(matches!(accounts.provision_owner_tenant(
            "Invalid Password","owner@site.test","short"),Err(AccountError::Invalid)));
        assert!(matches!(accounts.provision_owner_tenant(
            "Invalid Email","bad-email","long-valid-owner-password"),Err(AccountError::Invalid)));
        let count:i64=accounts.db.query_row(
            "SELECT COUNT(*) FROM network_tenants",[],|row|row.get(0)).unwrap();
        assert_eq!(count,2);
    }
    #[test]
    fn tenant_owner_login_is_hashed_and_revocable(){
        let (tmp,a,b,mut accounts)=init();
        let owner=accounts.bootstrap_owner(&a,"OWNER@EXAMPLE.COM","super-long-password-1").unwrap();
        assert_eq!(owner.email,"owner@example.com");
        assert!(accounts.bootstrap_owner(&a,"attacker@example.com","another-secret-234").is_err());
        assert!(accounts.sign_in(&a,"owner@example.com","wrong-password").is_err());
        let logged=accounts.sign_in(&a,"owner@example.com","super-long-password-1").unwrap();
        let debug_session=format!("{logged:?}");
        assert!(debug_session.contains("[REDACTED]"));
        assert!(!debug_session.contains(&logged.token));
        assert!(!debug_session.contains(&logged.principal.email));
        assert!(accounts.authorize(&logged.token,&a,Capability::ManageBilling).is_ok());
        assert!(accounts.authorize(&logged.token,&b,Capability::View).is_err());
        accounts.sign_out(&logged.token).unwrap();
        assert!(accounts.authorize(&logged.token,&a,Capability::View).is_err());
        let bytes=std::fs::read(tmp.path().join("control-plane.db")).unwrap();
        assert!(!bytes.windows("super-long-password-1".len())
            .any(|s|s==b"super-long-password-1"));
        assert!(!bytes.windows(logged.token.len()).any(|s|s==logged.token.as_bytes()));
    }
    #[test]
    fn invite_has_scoped_roles_and_single_use_acceptance(){
        let (_tmp,a,b,mut acc)=init();
        acc.bootstrap_owner(&a,"owner@example.com","super-long-password-2").unwrap();
        let owner=acc.sign_in(&a,"owner@example.com","super-long-password-2").unwrap();
        assert!(acc.invite_member(&owner.token,&b,"wrong@example.com",Role::Admin,600).is_err());
        assert!(acc.invite_member(&owner.token,&a,"other@example.com",Role::Owner,600).is_err());
        let invite=acc.invite_member(&owner.token,&a,"admin@example.com",Role::Admin,600).unwrap();
        let debug_invitation=format!("{invite:?}");
        assert!(debug_invitation.contains("[REDACTED]"));
        assert!(!debug_invitation.contains(&invite.token));
        let member=acc.accept_invitation(&invite.token,"my-admin-password-123").unwrap();
        assert_eq!(member.role,Role::Admin);
        assert!(acc.accept_invitation(&invite.token,"my-admin-password-123").is_err());
        let session=acc.sign_in(&a,"admin@example.com","my-admin-password-123").unwrap();
        assert!(acc.authorize(&session.token,&a,Capability::ManageSites).is_ok());
        assert!(acc.authorize(&session.token,&a,Capability::ManageBilling).is_err());
        assert!(acc.invite_member(&session.token,&a,"other-admin@example.com",Role::Admin,600).is_err());
        let viewer=acc.invite_member(&session.token,&a,"viewer@example.com",Role::Viewer,600).unwrap();
        acc.accept_invitation(&viewer.token,"viewer-secret-password").unwrap();
        let token=acc.sign_in(&a,"viewer@example.com","viewer-secret-password").unwrap();
        assert!(acc.authorize(&token.token,&a,Capability::View).is_ok());
        assert!(acc.authorize(&token.token,&a,Capability::ManageUsers).is_err());
    }
    #[test]
    fn membership_invite_and_accept_are_audited_atomically(){
        let (_tmp,tenant,_other,mut acc)=init();
        acc.bootstrap_owner(&tenant,"owner@example.com","valid-owner-password").unwrap();
        let owner=acc.sign_in(&tenant,"owner@example.com","valid-owner-password").unwrap();
        acc.db.execute_batch("
            CREATE TRIGGER deny_member_invited BEFORE INSERT ON network_audit
            WHEN NEW.action='member_invited'
            BEGIN SELECT RAISE(FAIL,'audit failed'); END;
        ").unwrap();
        assert!(acc.invite_member(&owner.token,&tenant,
            "member@example.com",Role::Viewer,600).is_err());
        let count:i64=acc.db.query_row(
            "SELECT COUNT(*) FROM network_member_invitations WHERE tenant_id=?1",
            [&tenant],|row|row.get(0)).unwrap();
        assert_eq!(count,0,"an unaudited invitation must not survive");
        acc.db.execute_batch("DROP TRIGGER deny_member_invited").unwrap();
        let invite=acc.invite_member(&owner.token,&tenant,
            "member@example.com",Role::Viewer,600).unwrap();
        let invitation_audit:(String,String)=acc.db.query_row(
            "SELECT action,subject_id FROM network_audit
             WHERE tenant_id=?1 AND action='member_invited'",
            [&tenant],|row|Ok((row.get(0)?,row.get(1)?))).unwrap();
        assert_eq!(invitation_audit.0,"member_invited");
        assert!(Uuid::parse_str(&invitation_audit.1).is_ok());
        assert_ne!(invitation_audit.1,invite.token);
        assert_ne!(invitation_audit.1,"member@example.com");
        acc.db.execute_batch("
            CREATE TRIGGER deny_member_joined BEFORE INSERT ON network_audit
            WHEN NEW.action='member_joined'
            BEGIN SELECT RAISE(FAIL,'audit failed'); END;
        ").unwrap();
        assert!(acc.accept_invitation(&invite.token,"valid-member-password").is_err());
        let users:i64=acc.db.query_row(
            "SELECT COUNT(*) FROM network_users WHERE tenant_id=?1 AND email='member@example.com'",
            [&tenant],|row|row.get(0)).unwrap();
        assert_eq!(users,0,"an unaudited member must not survive");
        let consumed:Option<String>=acc.db.query_row(
            "SELECT consumed_at FROM network_member_invitations
             WHERE tenant_id=?1 AND email='member@example.com'",
            [&tenant],|row|row.get(0)).unwrap();
        assert!(consumed.is_none(),"failed audit must preserve one-use invitation");
        acc.db.execute_batch("DROP TRIGGER deny_member_joined").unwrap();
        let member=acc.accept_invitation(&invite.token,"valid-member-password").unwrap();
        let subject:String=acc.db.query_row(
            "SELECT subject_id FROM network_audit
             WHERE tenant_id=?1 AND action='member_joined'",
            [&tenant],|row|row.get(0)).unwrap();
        assert_eq!(subject,member.user_id);
        assert!(acc.accept_invitation(&invite.token,"valid-member-password").is_err());
    }
    #[test]
    fn member_disable_revokes_sessions_is_tenant_scoped_and_audit_atomic(){
        let (_tmp,tenant,other,mut acc)=init();
        acc.bootstrap_owner(&tenant,"owner@example.com","long-owner-password").unwrap();
        acc.bootstrap_owner(&other,"other@example.com","long-other-password").unwrap();
        let owner=acc.sign_in(&tenant,"owner@example.com","long-owner-password").unwrap();
        let admin_inv=acc.invite_member(&owner.token,&tenant,
            "admin@example.com",Role::Admin,600).unwrap();
        let admin=acc.accept_invitation(&admin_inv.token,"long-admin-password").unwrap();
        let admin_session=acc.sign_in(&tenant,"admin@example.com","long-admin-password").unwrap();
        let viewer_inv=acc.invite_member(&owner.token,&tenant,
            "viewer@example.com",Role::Viewer,600).unwrap();
        let viewer=acc.accept_invitation(&viewer_inv.token,"long-viewer-password").unwrap();
        let viewer_session=acc.sign_in(&tenant,"viewer@example.com","long-viewer-password").unwrap();
        assert!(acc.disable_member(&viewer_session.token,&tenant,&admin.user_id).is_err());
        assert!(acc.disable_member(&admin_session.token,&tenant,&admin.user_id).is_err());
        assert!(acc.disable_member(&admin_session.token,&tenant,&owner.principal.user_id).is_err());
        assert!(acc.disable_member(&owner.token,&tenant,&owner.principal.user_id).is_err());
        assert!(acc.disable_member(&owner.token,&other,&viewer.user_id).is_err());
        acc.db.execute_batch("
            CREATE TRIGGER deny_disable_audit BEFORE INSERT ON network_audit
            WHEN NEW.action='member_disabled'
            BEGIN SELECT RAISE(FAIL,'audit unavailable'); END;
        ").unwrap();
        assert!(acc.disable_member(&admin_session.token,&tenant,&viewer.user_id).is_err());
        assert!(acc.authorize(&viewer_session.token,&tenant,Capability::View).is_ok(),
            "audit failure must roll back member and session state");
        acc.db.execute_batch("DROP TRIGGER deny_disable_audit").unwrap();
        acc.disable_member(&admin_session.token,&tenant,&viewer.user_id).unwrap();
        assert!(acc.authorize(&viewer_session.token,&tenant,Capability::View).is_err());
        assert!(acc.sign_in(&tenant,"viewer@example.com","long-viewer-password").is_err());
        let revoked:Option<String>=acc.db.query_row(
            "SELECT revoked_at FROM network_sessions
             WHERE user_id=?1 AND tenant_id=?2",
            params![&viewer.user_id,&tenant],|row|row.get(0)).unwrap();
        assert!(revoked.is_some());
        let audited:String=acc.db.query_row(
            "SELECT subject_id FROM network_audit
             WHERE tenant_id=?1 AND action='member_disabled'",
            [&tenant],|row|row.get(0)).unwrap();
        assert_eq!(audited,viewer.user_id);
        acc.disable_member(&owner.token,&tenant,&viewer.user_id).unwrap();
        let rows:i64=acc.db.query_row(
            "SELECT COUNT(*) FROM network_audit
             WHERE tenant_id=?1 AND action='member_disabled'",
            [&tenant],|row|row.get(0)).unwrap();
        assert_eq!(rows,1,"repeat disable must not duplicate audit events");
        acc.disable_member(&owner.token,&tenant,&admin.user_id).unwrap();
        assert!(acc.authorize(&admin_session.token,&tenant,Capability::View).is_err());
    }
    #[test]
    fn account_email_rejects_ascii_controls_and_whitespace(){
        for email in [
            "user\0@example.com",
            "user\x7f@example.com",
            "user\x0b@example.com",
            "user\n@example.com",
            "user\r@example.com",
            "user\t@example.com",
        ] {
            assert!(matches!(canonical_email(email),Err(AccountError::Invalid)));
        }
        assert_eq!(canonical_email("  USER@EXAMPLE.COM ").unwrap(),"user@example.com");
    }
    #[test]
    fn expired_lockout_resets_failure_counter_instead_of_instant_relock(){
        let (_tmp,tenant,_other,mut accounts)=init();
        accounts.bootstrap_owner(&tenant,"owner@example.com","correct-long-password").unwrap();
        for _ in 0..5 {
            assert!(accounts.sign_in(&tenant,"owner@example.com","wrong-password").is_err());
        }
        accounts.db.execute(
            "UPDATE network_users SET locked_until_unix=?1 WHERE tenant_id=?2",
            params![Utc::now().timestamp()-1,tenant]).unwrap();
        assert!(matches!(accounts.sign_in(&tenant,"owner@example.com","wrong-password"),
            Err(AccountError::Authentication)));
        let state:(i64,i64)=accounts.db.query_row(
            "SELECT failed_attempts,locked_until_unix FROM network_users WHERE tenant_id=?1",
            [&tenant],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!(state,(1,0));
        assert!(accounts.sign_in(&tenant,"owner@example.com","correct-long-password").is_ok());
    }
    #[test]
    fn lockout_fails_closed_without_disclosing_passwords(){
        let (_tmp,a,_b,mut acc)=init();
        acc.bootstrap_owner(&a,"owner@example.com","correct-long-password").unwrap();
        for _ in 0..5 {
            assert!(matches!(acc.sign_in(&a,"owner@example.com","incorrect-password"),Err(AccountError::Authentication)));
        }
        assert!(matches!(acc.sign_in(&a,"owner@example.com","correct-long-password"),Err(AccountError::Locked)));
    }
    #[test]
    fn tenant_member_inventory_enforces_roles_scoping_pagination_and_minimal_fields(){
        let (_tmp,tenant,other,mut acc)=init();
        acc.bootstrap_owner(&tenant,"owner@site.test","valid-owner-password").unwrap();
        acc.bootstrap_owner(&other,"other@site.test","valid-other-password").unwrap();
        let owner=acc.sign_in(&tenant,"owner@site.test","valid-owner-password").unwrap();
        let invitation=acc.invite_member(&owner.token,&tenant,
            "viewer@site.test",Role::Viewer,600).unwrap();
        let viewer_principal=acc.accept_invitation(&invitation.token,
            "valid-viewer-password").unwrap();
        let viewer=acc.sign_in(&tenant,"viewer@site.test","valid-viewer-password").unwrap();
        let members=acc.list_members(&owner.token,&tenant,100,0).unwrap();
        assert_eq!(members.len(),2);
        assert!(members.iter().all(|m|m.tenant_id==tenant));
        assert!(members.iter().any(|m|m.role==Role::Owner));
        assert!(members.iter().any(|m|m.user_id==viewer_principal.user_id));
        assert!(acc.list_members(&viewer.token,&tenant,10,0).is_err());
        assert!(acc.list_members(&owner.token,&other,10,0).is_err());
        assert!(matches!(acc.list_members(&owner.token,&tenant,101,0),Err(AccountError::Invalid)));
        assert!(matches!(acc.list_members(&owner.token,&tenant,1,-1),Err(AccountError::Invalid)));
        let page0=acc.list_members(&owner.token,&tenant,1,0).unwrap();
        let page1=acc.list_members(&owner.token,&tenant,1,1).unwrap();
        assert_ne!(page0[0].user_id,page1[0].user_id);
        assert!(acc.list_members(&owner.token,&tenant,1,2).unwrap().is_empty());
        acc.disable_member(&owner.token,&tenant,&viewer_principal.user_id).unwrap();
        let disabled=acc.list_members(&owner.token,&tenant,10,0).unwrap();
        assert_eq!(disabled.iter().find(|m|m.user_id==viewer_principal.user_id)
            .unwrap().status,"disabled");
        let item=serde_json::to_value(&disabled[0]).unwrap();
        assert_eq!(item.as_object().unwrap().len(),6);
        for key in ["passwordHash","salt","token","failedAttempts","lockedUntilUnix"] {
            assert!(item.get(key).is_none());
        }
    }
    #[test]
    fn password_change_revokes_all_sessions_and_never_leaks_credentials(){
        let (tmp,tenant,other,mut acc)=init();
        acc.bootstrap_owner(&tenant,"owner@site.test","initial-owner-password").unwrap();
        let session1=acc.sign_in(&tenant,"owner@site.test","initial-owner-password").unwrap();
        let session2=acc.sign_in(&tenant,"owner@site.test","initial-owner-password").unwrap();
        assert!(acc.change_password(&session1.token,&other,
            "initial-owner-password","replacement-owner-password").is_err());
        assert!(matches!(acc.change_password(&session1.token,&tenant,
            "wrong-current-password","replacement-owner-password"),
            Err(AccountError::Authentication)));
        assert!(matches!(acc.change_password(&session1.token,&tenant,
            "initial-owner-password","initial-owner-password"),
            Err(AccountError::Invalid)));
        assert!(acc.authorize(&session2.token,&tenant,Capability::View).is_ok());
        acc.change_password(&session1.token,&tenant,
            "initial-owner-password","replacement-owner-password").unwrap();
        assert!(acc.authorize(&session1.token,&tenant,Capability::View).is_err());
        assert!(acc.authorize(&session2.token,&tenant,Capability::View).is_err());
        assert!(acc.sign_in(&tenant,"owner@site.test","initial-owner-password").is_err());
        let new_session=acc.sign_in(&tenant,"owner@site.test","replacement-owner-password").unwrap();
        assert!(acc.authorize(&new_session.token,&tenant,Capability::View).is_ok());
        let subject:String=acc.db.query_row(
            "SELECT subject_id FROM network_audit
             WHERE tenant_id=?1 AND action='password_changed'",
            [&tenant],|row|row.get(0)).unwrap();
        assert_eq!(subject,session1.principal.user_id);
        let file=std::fs::read(tmp.path().join("control-plane.db")).unwrap();
        for secret in ["initial-owner-password","replacement-owner-password",
            session1.token.as_str(),session2.token.as_str()] {
            assert!(!file.windows(secret.len()).any(|window|window==secret.as_bytes()));
        }
    }
    #[test]
    fn password_change_audit_failure_rolls_back_password_and_session_revocation(){
        let (_tmp,tenant,_other,mut acc)=init();
        acc.bootstrap_owner(&tenant,"owner@site.test","initial-owner-password").unwrap();
        let session=acc.sign_in(&tenant,"owner@site.test","initial-owner-password").unwrap();
        acc.db.execute_batch(
            "CREATE TRIGGER deny_password_audit BEFORE INSERT ON network_audit
             WHEN NEW.action='password_changed'
             BEGIN SELECT RAISE(FAIL,'simulated audit failure'); END;"
        ).unwrap();
        assert!(acc.change_password(&session.token,&tenant,
            "initial-owner-password","replacement-owner-password").is_err());
        assert!(acc.authorize(&session.token,&tenant,Capability::View).is_ok());
        assert!(acc.sign_in(&tenant,"owner@site.test","replacement-owner-password").is_err());
        assert!(acc.sign_in(&tenant,"owner@site.test","initial-owner-password").is_ok());
        acc.db.execute_batch("DROP TRIGGER deny_password_audit").unwrap();
        acc.change_password(&session.token,&tenant,
            "initial-owner-password","replacement-owner-password").unwrap();
    }
    #[test]
    fn active_session_inventory_and_revocation_are_self_scoped_and_secret_free(){
        let (_tmp,tenant,other,mut acc)=init();
        acc.bootstrap_owner(&tenant,"owner@site.test","valid-owner-password").unwrap();
        acc.bootstrap_owner(&other,"other@site.test","valid-other-password").unwrap();
        let first=acc.sign_in(&tenant,"owner@site.test","valid-owner-password").unwrap();
        let second=acc.sign_in(&tenant,"owner@site.test","valid-owner-password").unwrap();
        let foreign=acc.sign_in(&other,"other@site.test","valid-other-password").unwrap();
        let rows=acc.list_sessions(&first.token,&tenant,10,0).unwrap();
        assert_eq!(rows.len(),2);
        assert_eq!(rows.iter().filter(|s|s.is_current).count(),1);
        assert!(acc.list_sessions(&first.token,&other,10,0).is_err());
        assert!(matches!(acc.list_sessions(&first.token,&tenant,101,0),Err(AccountError::Invalid)));
        assert_eq!(acc.list_sessions(&first.token,&tenant,1,0).unwrap().len(),1);
        assert_eq!(acc.list_sessions(&first.token,&tenant,1,1).unwrap().len(),1);
        let serialized=serde_json::to_string(&rows).unwrap();
        assert!(!serialized.contains(&first.token));
        assert!(!serialized.contains(&second.token));
        assert!(!serialized.contains("tokenHash"));
        let foreign_session=acc.list_sessions(&foreign.token,&other,10,0).unwrap();
        assert!(acc.revoke_session(&first.token,&tenant,&foreign_session[0].session_id).is_err());
        let other_id=rows.iter().find(|s|!s.is_current).unwrap().session_id.clone();
        acc.revoke_session(&first.token,&tenant,&other_id).unwrap();
        assert!(acc.authorize(&second.token,&tenant,Capability::View).is_err());
        assert!(acc.authorize(&first.token,&tenant,Capability::View).is_ok());
        assert_eq!(acc.list_sessions(&first.token,&tenant,10,0).unwrap().len(),1);
        let current_id=acc.list_sessions(&first.token,&tenant,10,0).unwrap()[0].session_id.clone();
        acc.revoke_session(&first.token,&tenant,&current_id).unwrap();
        assert!(acc.authorize(&first.token,&tenant,Capability::View).is_err());
        assert!(acc.authorize(&foreign.token,&other,Capability::View).is_ok());
        let audits:i64=acc.db.query_row(
            "SELECT COUNT(*) FROM network_audit
             WHERE tenant_id=?1 AND action='session_revoked'",
            [&tenant],|r|r.get(0)).unwrap();
        assert_eq!(audits,2);
    }
    #[test]
    fn session_revoke_audit_failure_leaves_session_valid(){
        let (_tmp,tenant,_other,mut acc)=init();
        acc.bootstrap_owner(&tenant,"owner@site.test","valid-owner-password").unwrap();
        let actor=acc.sign_in(&tenant,"owner@site.test","valid-owner-password").unwrap();
        let session_id=acc.list_sessions(&actor.token,&tenant,10,0).unwrap()[0].session_id.clone();
        acc.db.execute_batch(
            "CREATE TRIGGER deny_revoke_audit BEFORE INSERT ON network_audit
             WHEN NEW.action='session_revoked'
             BEGIN SELECT RAISE(FAIL,'audit failed'); END;"
        ).unwrap();
        assert!(acc.revoke_session(&actor.token,&tenant,&session_id).is_err());
        assert!(acc.authorize(&actor.token,&tenant,Capability::View).is_ok());
        acc.db.execute_batch("DROP TRIGGER deny_revoke_audit").unwrap();
        acc.revoke_session(&actor.token,&tenant,&session_id).unwrap();
        assert!(acc.authorize(&actor.token,&tenant,Capability::View).is_err());
    }
}
