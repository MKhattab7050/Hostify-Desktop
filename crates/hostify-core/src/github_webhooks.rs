//! GitHub Push webhook authentication for future opt-in deployments.
//! This validates signatures and strict repository/branch allowlists only.
//! It does not fetch code, run commands, mutate DNS or authorize execution.
use serde::{Deserialize,Serialize};
use sha2::{Digest,Sha256};
use thiserror::Error;
use uuid::Uuid;

const MAX_BODY:usize=1_048_576;
const SHA256_BLOCK:usize=64;
#[derive(Debug,Error,PartialEq,Eq)]
pub enum WebhookError {
    #[error("missing or invalid GitHub webhook signature")]
    InvalidSignature,
    #[error("GitHub push event is not permitted")]
    UnsupportedEvent,
    #[error("repository or branch does not match the approved deployment source")]
    NotAllowed,
    #[error("malformed or oversized webhook payload")]
    InvalidPayload,
    #[error("this webhook does not describe a new safe branch commit")]
    Ignored,
}
type Result<T>=std::result::Result<T,WebhookError>;
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
pub struct VerifiedPush {
    pub delivery_id:String,
    pub repository:String,
    pub branch:String,
    pub commit_sha:String,
}
fn hmac_sha256(secret:&[u8],message:&[u8])->[u8;32]{
    let mut key=[0u8;SHA256_BLOCK];
    if secret.len()>SHA256_BLOCK {
        key[..32].copy_from_slice(&Sha256::digest(secret));
    } else {key[..secret.len()].copy_from_slice(secret);}
    let mut inner=[0x36u8;SHA256_BLOCK];
    let mut outer=[0x5cu8;SHA256_BLOCK];
    for i in 0..SHA256_BLOCK {inner[i]^=key[i];outer[i]^=key[i];}
    let mut i=Sha256::new();i.update(inner);i.update(message);
    let digest=i.finalize();
    let mut o=Sha256::new();o.update(outer);o.update(digest);
    let value=o.finalize();
    let mut result=[0u8;32];result.copy_from_slice(&value);result
}
fn valid_signature(header:&str,body:&[u8],secret:&[u8])->bool{
    // Reject weak/empty configuration rather than accepting default secrets.
    if secret.len()<32 || !header.starts_with("sha256=") || header.len()!=71 {return false;}
    let Ok(expected)=hex::decode(&header[7..]) else {return false};
    let actual=hmac_sha256(secret,body);
    let mut diff=0u8;
    for (x,y) in actual.iter().zip(expected.iter()){diff |= x ^ y;}
    diff==0
}
fn valid_commit(sha:&str)->bool {
    (sha.len()==40 || sha.len()==64) &&
        sha.bytes().all(|c|c.is_ascii_hexdigit()) &&
        sha.bytes().any(|b|b!=b'0')
}
fn valid_branch(branch:&str)->bool {
    !branch.is_empty() && branch.len()<=200 &&
        !branch.starts_with('/') && !branch.ends_with('/') &&
        !branch.contains("..") && !branch.contains("//") &&
        branch.bytes().all(|b|b.is_ascii_alphanumeric() ||
            matches!(b,b'-'|b'_'|b'/'|b'.'))
}
pub fn verify_push(event:&str,delivery_id:&str,signature:&str,body:&[u8],secret:&[u8],
    expected_repository:&str,expected_branch:&str)->Result<VerifiedPush>{
    // Never process potentially attacker-crafted payloads before authenticity.
    if body.len()>MAX_BODY {return Err(WebhookError::InvalidPayload);}
    if !valid_signature(signature,body,secret) {
        return Err(WebhookError::InvalidSignature);
    }
    if event!="push" {return Err(WebhookError::UnsupportedEvent);}
    let delivery_id=Uuid::parse_str(delivery_id).map_err(|_|WebhookError::InvalidPayload)?.to_string();
    if !valid_branch(expected_branch) || expected_repository.trim().is_empty() ||
        expected_repository.contains([' ','\n','\r']) {
        return Err(WebhookError::NotAllowed);
    }
    let value:serde_json::Value=serde_json::from_slice(body).map_err(|_|WebhookError::InvalidPayload)?;
    let repository=value["repository"]["full_name"].as_str().ok_or(WebhookError::InvalidPayload)?;
    let branch=value["ref"].as_str().ok_or(WebhookError::InvalidPayload)?;
    let sha=value["after"].as_str().ok_or(WebhookError::InvalidPayload)?;
    if value["repository"]["fork"]==true || value["forced"]==true || value["deleted"]==true {
        return Err(WebhookError::Ignored);
    }
    if !repository.eq_ignore_ascii_case(expected_repository) ||
        branch!=format!("refs/heads/{expected_branch}") {
        return Err(WebhookError::NotAllowed);
    }
    if !valid_commit(sha) || value["before"].as_str()==Some(sha) {
        return Err(WebhookError::Ignored);
    }
    Ok(VerifiedPush{delivery_id,repository:repository.into(),
        branch:expected_branch.into(),commit_sha:sha.to_ascii_lowercase()})
}


/// A durable anti-replay claim for a push that has already passed HMAC and
/// allowlist verification. Claiming is not authorization to fetch or execute
/// code. Store the claim in the central controller's SQLite connection.
#[derive(Debug,Clone,Copy,PartialEq,Eq)]
pub enum DeliveryClaim { Fresh, Duplicate }

pub fn claim_delivery(
    db:&mut rusqlite::Connection,
    push:&VerifiedPush,
) -> std::result::Result<DeliveryClaim,rusqlite::Error> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS github_verified_deliveries (
        delivery_id TEXT PRIMARY KEY,
        repository TEXT NOT NULL,
        branch TEXT NOT NULL,
        commit_sha TEXT NOT NULL,
        accepted_at TEXT NOT NULL
    );")?;
    let tx=db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let inserted=tx.execute(
        "INSERT OR IGNORE INTO github_verified_deliveries
          (delivery_id,repository,branch,commit_sha,accepted_at)
          VALUES (?1,?2,?3,?4,?5)",
        rusqlite::params![push.delivery_id,push.repository,push.branch,push.commit_sha,
            chrono::Utc::now().to_rfc3339()],
    )?;
    tx.commit()?;
    Ok(if inserted==1 {DeliveryClaim::Fresh} else {DeliveryClaim::Duplicate})
}


/// Explicit opt-in policy for an existing, locally-owned website. Staging a
/// job is only a request for human review; nothing here downloads or runs code.
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
pub struct SourcePolicy {
    pub site_id:String,
    pub repository:String,
    pub branch:String,
    pub enabled:bool,
}
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
pub struct PendingPush {
    pub delivery_id:String,
    pub site_id:String,
    pub repository:String,
    pub branch:String,
    pub commit_sha:String,
    pub state:String,
}
#[derive(Debug,Clone,Copy,PartialEq,Eq)]
pub enum IntakeOutcome { Queued, Duplicate, Disabled, CapacityReached }

/// A real authenticated site owner must be verified by the *caller* before
/// this function. In particular, GitHub's signed webhook is not authorization
/// to change the user's deployment settings.
pub fn configure_source(
    db:&mut rusqlite::Connection,
    policy:&SourcePolicy
)->std::result::Result<(),rusqlite::Error>{
    let site=Uuid::parse_str(&policy.site_id).map_err(|_|rusqlite::Error::InvalidQuery)?;
    if !valid_branch(&policy.branch) ||
        policy.repository.split('/').count()!=2 ||
        policy.repository.len()>200 ||
        !policy.repository.bytes().all(|b|
            b.is_ascii_alphanumeric()||matches!(b,b'_'|b'-'|b'.'|b'/')) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    create_inbox_tables(db)?;
    let tx=db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let existing:Option<(String,String,bool)>={
        use rusqlite::OptionalExtension;
        tx.query_row(
            "SELECT repository,branch,enabled FROM github_site_sources WHERE site_id=?1",
            [site.to_string()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))
        ).optional()?
    };
    tx.execute(
        "INSERT INTO github_site_sources(site_id,repository,branch,enabled)
         VALUES(?1,?2,?3,?4)
         ON CONFLICT(site_id) DO UPDATE SET
           repository=excluded.repository,
           branch=excluded.branch,
           enabled=excluded.enabled",
        rusqlite::params![site.to_string(),policy.repository,policy.branch,policy.enabled]
    )?;
    // Changing the approved source or opting out revokes all outstanding
    // approvals, atomically, so stale commits cannot be reviewed later.
    let source_changed=existing.is_some_and(|(repo,branch,_)|
        !repo.eq_ignore_ascii_case(&policy.repository) || branch!=policy.branch);
    if source_changed || !policy.enabled {
        tx.execute(
            "UPDATE github_push_inbox SET state='rejected'
             WHERE site_id=?1 AND state='awaiting_review'",
            [site.to_string()]
        )?;
    }
    tx.commit()?;
    Ok(())
}
fn create_inbox_tables(db:&rusqlite::Connection)->std::result::Result<(),rusqlite::Error>{
    db.execute_batch("CREATE TABLE IF NOT EXISTS github_site_sources(
        site_id TEXT PRIMARY KEY, repository TEXT NOT NULL,
        branch TEXT NOT NULL, enabled INTEGER NOT NULL CHECK(enabled IN (0,1))
    );
    CREATE TABLE IF NOT EXISTS github_push_inbox(
        delivery_id TEXT PRIMARY KEY, site_id TEXT NOT NULL,
        repository TEXT NOT NULL, branch TEXT NOT NULL,
        commit_sha TEXT NOT NULL, state TEXT NOT NULL DEFAULT 'awaiting_review'
            CHECK(state IN ('awaiting_review','rejected','approved')),
        received_at TEXT NOT NULL,
        FOREIGN KEY(site_id) REFERENCES github_site_sources(site_id)
    );
    CREATE INDEX IF NOT EXISTS github_push_pending_by_site
        ON github_push_inbox(site_id,state);")?;
    Ok(())
}
/// Must be called only AFTER HMAC and exact repo/branch validation, via
/// verify_push(...). The pending queue survives restart and has a bounded
/// per-site backlog. No build, DNS mutation, or trust acknowledgment follows.
pub fn enqueue_verified_push(
    db:&mut rusqlite::Connection,
    site_id:&str,
    push:&VerifiedPush
)->std::result::Result<IntakeOutcome,rusqlite::Error>{
    let site_id=Uuid::parse_str(site_id).map_err(|_|rusqlite::Error::InvalidQuery)?.to_string();
    if !valid_branch(&push.branch) || !valid_commit(&push.commit_sha) ||
        Uuid::parse_str(&push.delivery_id).is_err() {
        return Err(rusqlite::Error::InvalidQuery);
    }
    create_inbox_tables(db)?;
    let tx=db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let policy:Option<(String,String,bool)>={
        use rusqlite::OptionalExtension;
        tx.query_row(
            "SELECT repository,branch,enabled FROM github_site_sources WHERE site_id=?1",
            [&site_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))
        ).optional()?
    };
    let Some((repository,branch,enabled))=policy else {return Ok(IntakeOutcome::Disabled)};
    if !enabled ||
        !push.repository.eq_ignore_ascii_case(&repository) ||
        push.branch!=branch {return Ok(IntakeOutcome::Disabled);}
    let duplicate:i64=tx.query_row(
        "SELECT COUNT(*) FROM github_push_inbox WHERE delivery_id=?1",
        [&push.delivery_id],|r|r.get(0))?;
    if duplicate!=0 {return Ok(IntakeOutcome::Duplicate);}
    let outstanding:i64=tx.query_row(
        "SELECT COUNT(*) FROM github_push_inbox
         WHERE site_id=?1 AND state='awaiting_review'",
        [&site_id],|r|r.get(0))?;
    if outstanding>=10 {return Ok(IntakeOutcome::CapacityReached);}
    tx.execute(
        "INSERT INTO github_push_inbox(delivery_id,site_id,repository,branch,commit_sha,received_at)
            VALUES(?1,?2,?3,?4,?5,?6)",
        rusqlite::params![push.delivery_id,site_id,push.repository,push.branch,push.commit_sha,
            chrono::Utc::now().to_rfc3339()]
    )?;
    tx.commit()?;
    Ok(IntakeOutcome::Queued)
}
/// Read-only history; user code and webhook secrets are never included.
pub fn pending_pushes(
    db:&rusqlite::Connection, site_id:&str
)->std::result::Result<Vec<PendingPush>,rusqlite::Error>{
    let id=Uuid::parse_str(site_id).map_err(|_|rusqlite::Error::InvalidQuery)?.to_string();
    create_inbox_tables(db)?;
    let mut stmt=db.prepare(
        "SELECT delivery_id,site_id,repository,branch,commit_sha,state
         FROM github_push_inbox WHERE site_id=?1 ORDER BY received_at DESC LIMIT 50"
    )?;
    stmt.query_map([id],|r|Ok(PendingPush{
        delivery_id:r.get(0)?,site_id:r.get(1)?,repository:r.get(2)?,
        branch:r.get(3)?,commit_sha:r.get(4)?,state:r.get(5)?,
    }))?.collect()
}
/// A site owner may reject a proposed push without running the code.
/// The worker remains intentionally disabled until isolated build+trust
/// approval and deployment safety mechanisms exist.
pub fn reject_pending_push(
    db:&mut rusqlite::Connection,site_id:&str,delivery_id:&str
)->std::result::Result<bool,rusqlite::Error>{
    let id=Uuid::parse_str(site_id).map_err(|_|rusqlite::Error::InvalidQuery)?.to_string();
    let delivery=Uuid::parse_str(delivery_id).map_err(|_|rusqlite::Error::InvalidQuery)?.to_string();
    create_inbox_tables(db)?;
    let changed=db.execute(
        "UPDATE github_push_inbox SET state='rejected'
         WHERE site_id=?1 AND delivery_id=?2 AND state='awaiting_review'",
        rusqlite::params![id,delivery]
    )?;
    Ok(changed==1)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rfc4231_hmac_sha256_vector(){
        let digest=hmac_sha256(&[0x0b;20],b"Hi There");
        assert_eq!(hex::encode(digest),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7");
    }
    fn fixture()->(Vec<u8>,String){
        let payload=serde_json::json!({"repository":{"full_name":"company/project","fork":false},
            "ref":"refs/heads/main","after":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "before":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "deleted":false,"forced":false});
        let body=payload.to_string().into_bytes();
        let sig=format!("sha256={}",hex::encode(hmac_sha256(&[42u8;32],&body)));
        (body,sig)
    }
    fn verify(body:&[u8],sig:&str,repo:&str,branch:&str)->Result<VerifiedPush>{
        verify_push("push","12345678-1234-4234-8234-123456789abc",sig,body,
            &[42u8;32],repo,branch)
    }
    #[test]
    fn authenticated_push_is_pinned_to_repo_branch_and_commit(){
        let (body,sig)=fixture();
        let result=verify(&body,&sig,"company/project","main").unwrap();
        assert_eq!(result.commit_sha,"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        assert_eq!(result.branch,"main");
        assert_eq!(verify(&body,&sig,"other/project","main").unwrap_err(),WebhookError::NotAllowed);
        assert_eq!(verify(&body,&sig,"company/project","dev").unwrap_err(),WebhookError::NotAllowed);
        let mut edited=body.clone();edited.push(32);
        assert_eq!(verify(&edited,&sig,"company/project","main").unwrap_err(),WebhookError::InvalidSignature);
    }
    #[test]
    fn signed_deletes_forced_updates_and_forks_are_ignored(){
        let (body,_)=fixture();
        for (field,value) in [("deleted",true),("forced",true)] {
            let mut data:serde_json::Value=serde_json::from_slice(&body).unwrap();
            data[field]=serde_json::json!(value);
            let payload=data.to_string().into_bytes();
            let sig=format!("sha256={}",hex::encode(hmac_sha256(&[42u8;32],&payload)));
            assert_eq!(verify(&payload,&sig,"company/project","main").unwrap_err(),WebhookError::Ignored);
        }
    }
    #[test]
    fn same_delivery_can_be_claimed_only_once_and_survives_reopen() {
        let dir=tempfile::tempdir().unwrap();
        let database=dir.path().join("deliveries.db");
        let (payload,sig)=fixture();
        let push=verify(&payload,&sig,"company/project","main").unwrap();
        {
            let mut db=rusqlite::Connection::open(&database).unwrap();
            assert_eq!(claim_delivery(&mut db,&push).unwrap(),DeliveryClaim::Fresh);
            assert_eq!(claim_delivery(&mut db,&push).unwrap(),DeliveryClaim::Duplicate);
        }
        let mut db=rusqlite::Connection::open(&database).unwrap();
        assert_eq!(claim_delivery(&mut db,&push).unwrap(),DeliveryClaim::Duplicate);
    }
    #[test]
    fn webhook_inbox_requires_explicit_opt_in_and_preserves_review_gate() {
        let tmp=tempfile::tempdir().unwrap();
        let path=tmp.path().join("inbox.db");
        let site=Uuid::new_v4().to_string();
        let (body,sig)=fixture();
        let push=verify(&body,&sig,"company/project","main").unwrap();
        {
            let mut db=rusqlite::Connection::open(&path).unwrap();
            assert_eq!(enqueue_verified_push(&mut db,&site,&push).unwrap(),IntakeOutcome::Disabled);
            configure_source(&mut db,&SourcePolicy{
                site_id:site.clone(),repository:"company/project".into(),
                branch:"main".into(),enabled:true,
            }).unwrap();
            assert_eq!(enqueue_verified_push(&mut db,&site,&push).unwrap(),IntakeOutcome::Queued);
            assert_eq!(enqueue_verified_push(&mut db,&site,&push).unwrap(),IntakeOutcome::Duplicate);
            let list=pending_pushes(&db,&site).unwrap();
            assert_eq!(list.len(),1);
            assert_eq!(list[0].state,"awaiting_review");
        }
        let mut db=rusqlite::Connection::open(&path).unwrap();
        assert_eq!(pending_pushes(&db,&site).unwrap().len(),1);
        assert!(reject_pending_push(&mut db,&site,&push.delivery_id).unwrap());
        assert!(!reject_pending_push(&mut db,&site,&push.delivery_id).unwrap());
        assert_eq!(pending_pushes(&db,&site).unwrap()[0].state,"rejected");
    }
    #[test]
    fn source_mismatch_disable_and_backlog_are_fail_closed() {
        let mut db=rusqlite::Connection::open_in_memory().unwrap();
        let id=Uuid::new_v4().to_string();
        configure_source(&mut db,&SourcePolicy{site_id:id.clone(),
            repository:"org/site".into(),branch:"main".into(),enabled:true}).unwrap();
        let denied=VerifiedPush{delivery_id:Uuid::new_v4().to_string(),
            repository:"other/repo".into(),branch:"main".into(),
            commit_sha:"a".repeat(40)};
        assert_eq!(enqueue_verified_push(&mut db,&id,&denied).unwrap(),IntakeOutcome::Disabled);
        let mut push=denied;
        push.repository="org/site".into();
        for i in 0..10 {
            push.delivery_id=Uuid::new_v4().to_string();
            push.commit_sha=format!("{i:040x}");
            if i==0 {push.commit_sha="b".repeat(40);}
            assert_eq!(enqueue_verified_push(&mut db,&id,&push).unwrap(),IntakeOutcome::Queued);
        }
        push.delivery_id=Uuid::new_v4().to_string();
        push.commit_sha="c".repeat(40);
        assert_eq!(enqueue_verified_push(&mut db,&id,&push).unwrap(),IntakeOutcome::CapacityReached);
        configure_source(&mut db,&SourcePolicy{site_id:id.clone(),
            repository:"org/site".into(),branch:"main".into(),enabled:false}).unwrap();
        assert_eq!(enqueue_verified_push(&mut db,&id,&push).unwrap(),IntakeOutcome::Disabled);
    }
    #[test]
    fn opting_out_or_changing_repository_revokes_pending_commits_atomically() {
        let mut db=rusqlite::Connection::open_in_memory().unwrap();
        let id=Uuid::new_v4().to_string();
        let policy=SourcePolicy{site_id:id.clone(),repository:"org/site".into(),
            branch:"main".into(),enabled:true};
        configure_source(&mut db,&policy).unwrap();
        let mut push=VerifiedPush{delivery_id:Uuid::new_v4().to_string(),
            repository:"org/site".into(),branch:"main".into(),commit_sha:"a".repeat(40)};
        assert_eq!(enqueue_verified_push(&mut db,&id,&push).unwrap(),IntakeOutcome::Queued);
        configure_source(&mut db,&SourcePolicy{enabled:false,..policy.clone()}).unwrap();
        assert_eq!(pending_pushes(&db,&id).unwrap()[0].state,"rejected");
        configure_source(&mut db,&policy).unwrap();
        push.delivery_id=Uuid::new_v4().to_string();
        assert_eq!(enqueue_verified_push(&mut db,&id,&push).unwrap(),IntakeOutcome::Queued);
        configure_source(&mut db,&SourcePolicy{repository:"org/another".into(),..policy}).unwrap();
        assert!(pending_pushes(&db,&id).unwrap().iter().all(|item|item.state=="rejected"));
    }
    #[test]
    fn malformed_input_fails_closed(){
        let (body,sig)=fixture();
        assert_eq!(verify_push("ping","wrong",&sig,&body,&[42;32],"company/project","main").unwrap_err(),WebhookError::UnsupportedEvent);
    }
}
