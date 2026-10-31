//! Local-only tenant account endpoints for the trusted development control
//! plane. This is not an Internet-facing identity service: TLS, MFA, rate
//! limiting, and tenant gateways must be independently implemented first.
use hostify_core::network_auth::{AccountError,Accounts,Capability,Principal,Role};
use hyper::StatusCode;
use serde::Deserialize;
use serde_json::{json,Value};
use std::sync::Mutex;

pub struct AccountApi{
    accounts:Mutex<Accounts>,
}
impl AccountApi{
    pub fn new(accounts:Accounts)->Self{Self{accounts:Mutex::new(accounts)}}

    pub fn authorize(&self,auth:Option<&str>,tenant:&str,cap:Capability)
        ->Result<Principal,(StatusCode,&'static str)>{
        let token=auth.and_then(|h|h.strip_prefix("Bearer "))
            .ok_or((StatusCode::UNAUTHORIZED,"unauthorized"))?;
        let accounts=self.accounts.lock()
            .map_err(|_|(StatusCode::SERVICE_UNAVAILABLE,"unavailable"))?;
        accounts.authorize(token,tenant,cap).map_err(map_account_error)
    }

    pub fn dispatch(&self,path:&str,auth:Option<&str>,admin:bool,body:&[u8])
        ->(StatusCode,Value){
        fn parse<T:serde::de::DeserializeOwned>(body:&[u8])
            ->Result<T,(StatusCode,&'static str)>{
            serde_json::from_slice(body).map_err(|_|(StatusCode::BAD_REQUEST,"invalid_json"))
        }
        fn session_token(auth:Option<&str>)->Result<&str,(StatusCode,&'static str)>{
            auth.and_then(|header|header.strip_prefix("Bearer "))
                .ok_or((StatusCode::UNAUTHORIZED,"unauthorized"))
        }
        #[derive(Deserialize)]#[serde(rename_all="camelCase",deny_unknown_fields)]
        struct Provision {name:String,email:String,password:String}
        #[derive(Deserialize)]#[serde(rename_all="camelCase",deny_unknown_fields)]
        struct Bootstrap {tenant_id:String,email:String,password:String}
        #[derive(Deserialize)]#[serde(rename_all="camelCase",deny_unknown_fields)]
        struct Login {tenant_id:String,email:String,password:String}
        #[derive(Deserialize)]#[serde(rename_all="camelCase",deny_unknown_fields)]
        struct Me {tenant_id:String}
        #[derive(Deserialize)]#[serde(rename_all="camelCase",deny_unknown_fields)]
        struct Invite {tenant_id:String,email:String,role:Role,ttl_seconds:i64}
        #[derive(Deserialize)]#[serde(rename_all="camelCase",deny_unknown_fields)]
        struct Accept {invitation:String,password:String}
        let response:Result<Value,(StatusCode,&'static str)>=(||{
            let mut accounts=self.accounts.lock()
                .map_err(|_|(StatusCode::SERVICE_UNAVAILABLE,"unavailable"))?;
            match path {
                "/v1/accounts/provision"=>{
                    if !admin {return Err((StatusCode::UNAUTHORIZED,"unauthorized"));}
                    let p:Provision=parse(body)?;
                    let (tenant,owner)=accounts.provision_owner_tenant(
                        &p.name,&p.email,&p.password).map_err(map_account_error)?;
                    Ok(json!({"tenant":tenant,"owner":owner}))
                },
                "/v1/accounts/bootstrap"=>{
                    if !admin {return Err((StatusCode::UNAUTHORIZED,"unauthorized"));}
                    let p:Bootstrap=parse(body)?;
                    let principal=accounts.bootstrap_owner(&p.tenant_id,&p.email,&p.password)
                        .map_err(map_account_error)?;
                    Ok(json!({"owner":principal}))
                },
                "/v1/auth/login"=>{
                    let p:Login=parse(body)?;
                    let session=accounts.sign_in(&p.tenant_id,&p.email,&p.password)
                        .map_err(map_account_error)?;
                    Ok(json!({"token":session.token,"expiresUnix":session.expires_unix,
                        "user":session.principal}))
                },
                "/v1/auth/logout"=>{
                    accounts.sign_out(session_token(auth)?).map_err(map_account_error)?;
                    Ok(json!({"signedOut":true}))
                },
                "/v1/auth/me"=>{
                    let p:Me=parse(body)?;
                    let principal=accounts.authorize(session_token(auth)?,&p.tenant_id,Capability::View)
                        .map_err(map_account_error)?;
                    Ok(json!({"user":principal}))
                },
                "/v1/members/invite"=>{
                    let p:Invite=parse(body)?;
                    let invitation=accounts.invite_member(session_token(auth)?,&p.tenant_id,
                        &p.email,p.role,p.ttl_seconds).map_err(map_account_error)?;
                    Ok(json!({"invitation":invitation.token,"expiresUnix":invitation.expires_unix}))
                },
                "/v1/members/accept"=>{
                    let p:Accept=parse(body)?;
                    let member=accounts.accept_invitation(&p.invitation,&p.password)
                        .map_err(map_account_error)?;
                    Ok(json!({"member":member}))
                },
                _=>Err((StatusCode::NOT_FOUND,"not_found")),
            }
        })();
        match response{
            Ok(value)=>(StatusCode::OK,value),
            Err((status,message))=>(status,json!({"error":message})),
        }
    }
}

pub fn is_account_path(path:&str)->bool{
    matches!(path,"/v1/accounts/provision"|"/v1/accounts/bootstrap"|"/v1/auth/login"|
        "/v1/auth/logout"|"/v1/auth/me"|"/v1/members/invite"|"/v1/members/accept")
}
fn map_account_error(error:AccountError)->(StatusCode,&'static str){
    match error{
        AccountError::Invalid=>(StatusCode::BAD_REQUEST,"invalid_request"),
        AccountError::Authentication|AccountError::Invitation|AccountError::Unavailable=>
            (StatusCode::UNAUTHORIZED,"unauthorized"),
        AccountError::Locked=>(StatusCode::TOO_MANY_REQUESTS,"temporarily_locked"),
        AccountError::Forbidden=>(StatusCode::FORBIDDEN,"not_authorized"),
        AccountError::Database(_)|AccountError::Random=>
            (StatusCode::SERVICE_UNAVAILABLE,"temporarily_unavailable"),
    }
}
