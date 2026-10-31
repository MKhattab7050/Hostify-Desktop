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
    fn malformed_input_fails_closed(){
        let (body,sig)=fixture();
        assert_eq!(verify_push("ping","wrong",&sig,&body,&[42;32],"company/project","main").unwrap_err(),WebhookError::UnsupportedEvent);
    }
}
