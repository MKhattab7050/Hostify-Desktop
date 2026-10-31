//! Local Hostify Network Ed25519 device identity. The private key is never
//! serialized into site records, returned through IPC, or sent to the gateway.
//! The caller provides a protected SecretStore (Windows Credential Manager in
//! production); keys never fall back to a plaintext file on error.
use crate::secrets::{SecretError,SecretStore};
use ring::{rand::SystemRandom,signature::{Ed25519KeyPair,KeyPair}};
use serde::{Serialize,Deserialize};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug,Error)]
pub enum IdentityError {
    #[error("Device identity is invalid")]
    Invalid,
    #[error("Device private key is unavailable or corrupted")]
    KeyUnavailable,
    #[error("Device key generation failed")]
    Generation,
    #[error("Device identity credential store failed: {0}")]
    Secret(#[from] SecretError),
}
#[derive(Debug,Clone,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
pub struct DeviceIdentity {
    pub key_id:String,
    pub public_key_hex:String,
}
fn storage_key(key_id:&str)->Result<String,IdentityError>{
    let id=Uuid::parse_str(key_id).map_err(|_|IdentityError::Invalid)?;
    if id.to_string()!=key_id.to_ascii_lowercase(){return Err(IdentityError::Invalid);}
    Ok(format!("network-device-ed25519-v1:{}",id))
}
/// Explicitly creates a new device key; no implicit rotation or overwrite.
pub fn create_device_identity(secrets:&dyn SecretStore)->Result<DeviceIdentity,IdentityError>{
    let rng=SystemRandom::new();
    let pkcs8=Ed25519KeyPair::generate_pkcs8(&rng).map_err(|_|IdentityError::Generation)?;
    let pair=Ed25519KeyPair::from_pkcs8(pkcs8.as_ref())
        .map_err(|_|IdentityError::Generation)?;
    let id=Uuid::new_v4().to_string();
    secrets.set(&storage_key(&id)?,&hex::encode(pkcs8.as_ref()))?;
    Ok(DeviceIdentity{key_id:id,public_key_hex:hex::encode(pair.public_key().as_ref())})
}
pub fn public_identity(
    secrets:&dyn SecretStore,key_id:&str
)->Result<DeviceIdentity,IdentityError>{
    let key=secrets.get(&storage_key(key_id)?)?.ok_or(IdentityError::KeyUnavailable)?;
    let encoded=hex::decode(key).map_err(|_|IdentityError::KeyUnavailable)?;
    let pair=Ed25519KeyPair::from_pkcs8(&encoded).map_err(|_|IdentityError::KeyUnavailable)?;
    Ok(DeviceIdentity{key_id:key_id.to_owned(),
        public_key_hex:hex::encode(pair.public_key().as_ref())})
}
/// Only signs structured Hostify proof challenges for a pinned DNS namespace.
/// This is not a generic remote signing oracle.
pub fn sign_route_proof(
    secrets:&dyn SecretStore,key_id:&str,base_domain:&str,
    tenant_id:&str,agent_id:&str,route_id:&str,challenge:&str
)->Result<String,IdentityError>{
    if challenge.len()!=64 || !challenge.bytes().all(|b|b.is_ascii_hexdigit()) ||
        !base_domain.contains('.') || base_domain.len()>253 ||
        base_domain.split('.').any(|l|l.is_empty()||l.len()>63||
            !l.bytes().all(|c| c.is_ascii_lowercase()||c.is_ascii_digit()||c==b'-')) {
        return Err(IdentityError::Invalid);
    }
    for id in [tenant_id,agent_id,route_id] {
        if Uuid::parse_str(id).is_err(){return Err(IdentityError::Invalid);}
    }
    let name=storage_key(key_id)?;
    let secret=secrets.get(&name)?.ok_or(IdentityError::KeyUnavailable)?;
    let encoded=hex::decode(secret).map_err(|_|IdentityError::KeyUnavailable)?;
    let pair=Ed25519KeyPair::from_pkcs8(&encoded).map_err(|_|IdentityError::KeyUnavailable)?;
    let message=format!("HOSTIFY_DEVICE_ROUTE_PROOF_V1\n{}\n{}\n{}\n{}\n{}\n",
        base_domain,tenant_id,agent_id,route_id,challenge);
    Ok(hex::encode(pair.sign(message.as_bytes()).as_ref()))
}
/// Explicit device revocation. This destroys the local signing key only,
/// not the central registry's revocation status: revoke remotely first.
pub fn erase_local_identity(
    secrets:&dyn SecretStore,key_id:&str
)->Result<(),IdentityError>{
    secrets.delete(&storage_key(key_id)?)?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::MemoryStore;
    #[test]
    fn private_key_stays_in_secret_store_and_signed_proof_is_verifiable(){
        let store=MemoryStore::default();
        let device=create_device_identity(&store).unwrap();
        assert_eq!(device.public_key_hex.len(),64);
        assert_eq!(public_identity(&store,&device.key_id).unwrap().public_key_hex,device.public_key_hex);
        let tenant=Uuid::new_v4().to_string();
        let agent=Uuid::new_v4().to_string();
        let route=Uuid::new_v4().to_string();
        let challenge="a".repeat(64);
        let signed=sign_route_proof(&store,&device.key_id,"example.test",
            &tenant,&agent,&route,&challenge).unwrap();
        let message=format!("HOSTIFY_DEVICE_ROUTE_PROOF_V1\nexample.test\n{}\n{}\n{}\n{}\n",
            tenant,agent,route,challenge);
        ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519,
            hex::decode(&device.public_key_hex).unwrap())
            .verify(message.as_bytes(),&hex::decode(signed).unwrap()).unwrap();
        assert!(sign_route_proof(&store,&device.key_id,"example.test",
            &tenant,&agent,&route,"bad").is_err());
        assert!(sign_route_proof(&store,&device.key_id,"https://example.test",
            &tenant,&agent,&route,&challenge).is_err());
        erase_local_identity(&store,&device.key_id).unwrap();
        assert!(matches!(public_identity(&store,&device.key_id),
            Err(IdentityError::KeyUnavailable)));
    }
    #[test]
    fn broken_secret_store_never_generates_an_unprotected_private_key(){
        struct Unavailable;
        impl SecretStore for Unavailable {
            fn set(&self,_:&str,_:&str)->Result<(),SecretError>{Err(SecretError::Unavailable)}
            fn get(&self,_:&str)->Result<Option<String>,SecretError>{Err(SecretError::Unavailable)}
            fn delete(&self,_:&str)->Result<(),SecretError>{Err(SecretError::Unavailable)}
        }
        assert!(matches!(create_device_identity(&Unavailable),
            Err(IdentityError::Secret(SecretError::Unavailable))));
    }
}
