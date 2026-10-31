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
            let next=failures.saturating_add(1);
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
        self.db.execute("INSERT INTO network_member_invitations
            (id,tenant_id,email,role,inviter_id,token_hash,expires_unix,created_at)
            VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![Uuid::new_v4().to_string(),tenant,email,role.db(),
                inviter.user_id,digest(&pending_token),expires_unix,Utc::now().to_rfc3339()])?;
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
    fn lockout_fails_closed_without_disclosing_passwords(){
        let (_tmp,a,_b,mut acc)=init();
        acc.bootstrap_owner(&a,"owner@example.com","correct-long-password").unwrap();
        for _ in 0..5 {
            assert!(matches!(acc.sign_in(&a,"owner@example.com","incorrect-password"),Err(AccountError::Authentication)));
        }
        assert!(matches!(acc.sign_in(&a,"owner@example.com","correct-long-password"),Err(AccountError::Locked)));
    }
}
