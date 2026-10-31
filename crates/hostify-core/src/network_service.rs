//! Local, central-only composition of tenant authorization and the network
//! registry. There is deliberately no HTTP listener, Cloudflare DNS mutation,
 //! or site execution API here. Operator provisioning is not publicly exposed.
use crate::network::{Registry,NetworkError,Tenant,Invitation,Route,DeviceChallenge,Agent,AuditEntry};
use crate::network_auth::{Accounts,AccountError,Principal,Session,Role,Capability,MemberInvitation,TenantMember};
use std::path::Path;
use thiserror::Error;

#[derive(Debug,Error)]
pub enum ControlError {
    #[error(transparent)]
    Auth(#[from]AccountError),
    #[error(transparent)]
    Network(#[from]NetworkError),
}
type Result<T>=std::result::Result<T,ControlError>;

pub struct NetworkControlPlane{
    registry:Registry,
    accounts:Accounts,
}
impl NetworkControlPlane{
    pub fn open(path:&Path,base_domain:&str)->Result<Self>{
        // Run registry migration first so account foreign keys reference an
        // existing tenant table on the same SQLite database.
        let registry=Registry::open(path,base_domain)?;
        let accounts=Accounts::open(path)?;
        Ok(Self{registry,accounts})
    }
    /// Trusted installation/operator step only, never an internet endpoint.
    pub fn provision_tenant_operator(&mut self,name:&str,email:&str,password:&str)->Result<(Tenant,Principal)>{
        // The tenant, first owner account and corresponding audit event are
        // committed together. A failed account insert cannot orphan a tenant.
        Ok(self.accounts.provision_owner_tenant(name,email,password)?)
    }
    pub fn login(&mut self,tenant:&str,email:&str,password:&str)->Result<Session>{
        Ok(self.accounts.sign_in(tenant,email,password)?)
    }
    pub fn logout(&mut self,token:&str)->Result<()>{
        Ok(self.accounts.sign_out(token)?)
    }
    pub fn current_user(&self,token:&str,tenant:&str)->Result<Principal>{
        Ok(self.accounts.authorize(token,tenant,Capability::View)?)
    }
    pub fn invite_member(&mut self,token:&str,tenant:&str,email:&str,role:Role,ttl_seconds:i64)->Result<MemberInvitation>{
        Ok(self.accounts.invite_member(token,tenant,email,role,ttl_seconds)?)
    }
    pub fn accept_member_invitation(&mut self,token:&str,password:&str)->Result<Principal>{
        Ok(self.accounts.accept_invitation(token,password)?)
    }
    /// Management-only member roster with bounded, tenant-local pagination.
    pub fn list_members(&self,token:&str,tenant:&str,limit:i64,offset:i64)->Result<Vec<TenantMember>>{
        Ok(self.accounts.list_members(token,tenant,limit,offset)?)
    }
    /// Read-only inventory requires an active tenant session, including for
    /// viewers. Core Registry methods alone do not establish authorization.
    pub fn list_devices(&self,token:&str,tenant:&str,limit:i64,offset:i64)->Result<Vec<Agent>>{
        self.accounts.authorize(token,tenant,Capability::View)?;
        Ok(self.registry.list_tenant_agents(tenant,limit,offset)?)
    }
    pub fn list_routes(&self,token:&str,tenant:&str,limit:i64,offset:i64)->Result<Vec<Route>>{
        self.accounts.authorize(token,tenant,Capability::View)?;
        Ok(self.registry.list_tenant_routes(tenant,limit,offset)?)
    }
    /// Owner and administrators may inspect tenant-local security activity.
    /// View-only members cannot inspect the security audit stream.
    pub fn list_security_audit(&self,token:&str,tenant:&str,limit:i64,offset:i64)->Result<Vec<AuditEntry>>{
        self.accounts.authorize(token,tenant,Capability::ManageUsers)?;
        Ok(self.registry.list_tenant_audit(tenant,limit,offset)?)
    }
    pub fn issue_device_enrollment(&mut self,token:&str,tenant:&str,ttl_seconds:i64)->Result<Invitation>{
        self.accounts.authorize(token,tenant,Capability::ManageDevices)?;
        Ok(self.registry.issue_invitation(tenant,ttl_seconds)?)
    }
    /// Device enrollment is an out-of-band gateway action; the invitation
    /// alone cannot publish public traffic.
    pub fn enroll_device(&mut self,invitation_token:&str,public_key_hex:&str)->Result<Agent>{
        Ok(self.registry.enroll_agent(invitation_token,public_key_hex)?)
    }
    pub fn reserve_subdomain(&mut self,token:&str,tenant:&str,agent:&str,site:&str,label:&str)->Result<Route>{
        self.accounts.authorize(token,tenant,Capability::ManageSites)?;
        Ok(self.registry.reserve_hostname(tenant,agent,site,label)?)
    }
    pub fn issue_device_proof(&mut self,token:&str,tenant:&str,agent:&str,route:&str)->Result<DeviceChallenge>{
        self.accounts.authorize(token,tenant,Capability::ManageDevices)?;
        Ok(self.registry.issue_route_challenge(tenant,agent,route,120)?)
    }
    /// This method must be called only after the future ingress validates
    /// the sender/device context and applies per-device rate limits.
    pub fn verify_device_proof(&mut self,tenant:&str,agent:&str,route:&str,
        challenge:&str,signature:&str)->Result<()>{
        Ok(self.registry.verify_route_signature(tenant,agent,route,challenge,signature)?)
    }
    pub fn revoke_device(&mut self,token:&str,tenant:&str,agent:&str)->Result<()>{
        self.accounts.authorize(token,tenant,Capability::ManageDevices)?;
        Ok(self.registry.revoke_agent(tenant,agent)?)
    }
    /// Internal routing data cannot become live from enrollment or proof.
    pub fn public_route(&self,hostname:&str)->Result<Option<Route>>{
        Ok(self.registry.lookup_active_route(hostname)?)
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    use uuid::Uuid;
    #[test]
    fn account_permissions_guard_registry_and_identity_proof_never_activates_public_route(){
        use ring::{rand::SystemRandom,signature::{Ed25519KeyPair,KeyPair}};
        let dir=tempfile::tempdir().unwrap();
        let mut service=NetworkControlPlane::open(&dir.path().join("control.db"),"example.test").unwrap();
        let (tenant,_)=service.provision_tenant_operator("Tenant A","owner@site.test",
            "superlong-owner-password").unwrap();
        let (other,_)=service.provision_tenant_operator("Tenant B","other@site.test",
            "superlong-second-password").unwrap();
        let owner=service.login(&tenant.id,"owner@site.test","superlong-owner-password").unwrap();
        let viewer_inv=service.invite_member(&owner.token,&tenant.id,
            "viewer@site.test",Role::Viewer,600).unwrap();
        service.accept_member_invitation(&viewer_inv.token,"superlong-viewer-password").unwrap();
        let viewer=service.login(&tenant.id,"viewer@site.test","superlong-viewer-password").unwrap();
        let audit=service.list_security_audit(&owner.token,&tenant.id,10,0).unwrap();
        assert!(!audit.is_empty());
        assert!(audit.iter().all(|entry|entry.tenant_id==tenant.id));
        assert!(service.list_security_audit(&viewer.token,&tenant.id,10,0).is_err());
        assert!(service.list_security_audit(&owner.token,&other.id,10,0).is_err());
        assert!(service.list_security_audit(&owner.token,&tenant.id,101,0).is_err());
        assert!(service.issue_device_enrollment(&viewer.token,&tenant.id,300).is_err());
        assert!(service.issue_device_enrollment(&owner.token,&other.id,300).is_err());
        let rng=SystemRandom::new();
        let key=Ed25519KeyPair::from_pkcs8(Ed25519KeyPair::generate_pkcs8(&rng).unwrap().as_ref()).unwrap();
        let invitation=service.issue_device_enrollment(&owner.token,&tenant.id,300).unwrap();
        let agent=service.enroll_device(&invitation.token,&hex::encode(key.public_key().as_ref())).unwrap();
        let route=service.reserve_subdomain(&owner.token,&tenant.id,&agent.id,
            &Uuid::new_v4().to_string(),"tenant-a").unwrap();
        assert_eq!(service.list_devices(&viewer.token,&tenant.id,10,0).unwrap().len(),1);
        assert_eq!(service.list_routes(&viewer.token,&tenant.id,10,0).unwrap().len(),1);
        assert!(service.list_devices(&viewer.token,&other.id,10,0).is_err());
        assert!(service.list_routes(&owner.token,&other.id,10,0).is_err());
        assert!(service.reserve_subdomain(&viewer.token,&tenant.id,&agent.id,
            &Uuid::new_v4().to_string(),"denied").is_err());
        let challenge=service.issue_device_proof(&owner.token,&tenant.id,&agent.id,&route.id).unwrap();
        // Canonical format is the stable v1 contract between gateway and device.
        let message=format!("HOSTIFY_DEVICE_ROUTE_PROOF_V1\nexample.test\n{}\n{}\n{}\n{}\n",
            tenant.id,agent.id,route.id,challenge.value);
        let signature=hex::encode(key.sign(message.as_bytes()).as_ref());
        service.verify_device_proof(&tenant.id,&agent.id,&route.id,&challenge.value,&signature).unwrap();
        assert!(service.public_route(&route.hostname).unwrap().is_none());
        service.revoke_device(&owner.token,&tenant.id,&agent.id).unwrap();
        assert!(service.list_devices(&owner.token,&tenant.id,10,0).unwrap().is_empty());
        assert!(service.list_routes(&owner.token,&tenant.id,10,0).unwrap().is_empty());
        assert!(service.public_route(&route.hostname).unwrap().is_none());
    }
}
