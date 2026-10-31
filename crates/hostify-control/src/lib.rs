//! Hostify Network control plane: localhost-only administration and agent
//! enrollment gateway. No public networking, DNS changes or workload execution.
mod accounts_http;
use accounts_http::{AccountApi,is_account_path};
use bytes::Bytes;
use hostify_core::network::{NetworkError,Registry};
use hostify_core::network_auth::{Accounts,Capability};
use http_body_util::{Full,BodyExt};
use hyper::{Method,Request,Response,StatusCode,body::Incoming,header};
use ring::constant_time::verify_slices_are_equal;
use serde::Deserialize;
use serde_json::{json,Value};
use std::sync::{Arc,Mutex};
use uuid::Uuid;

const MAX_BODY:usize=32*1024;
pub struct Controller {
    registry:Mutex<Registry>,
    admin_secret:Vec<u8>,
    accounts:Option<AccountApi>,
}
impl Controller {
    pub fn new(registry:Registry,admin_secret:Vec<u8>)->Result<Self,&'static str>{
        if admin_secret.len()<32 || admin_secret.len()>4096 ||
            admin_secret.iter().any(|b| !b.is_ascii_graphic()) {
            return Err("Configure a random administrator secret of 32-4096 printable ASCII bytes.");
        }
        Ok(Self{registry:Mutex::new(registry),admin_secret,accounts:None})
    }
    /// Attach only an already-initialized central account database. This
    /// changes no localhost-only transport or public readiness state.
    pub fn with_accounts(mut self,accounts:Accounts)->Self{
        self.accounts=Some(AccountApi::new(accounts));
        self
    }
    fn authorized(&self,header_value:Option<&str>)->bool{
        let Some(token)=header_value.and_then(|h|h.strip_prefix("Bearer ")) else{return false;};
        token.len()==self.admin_secret.len() &&
            verify_slices_are_equal(token.as_bytes(),&self.admin_secret).is_ok()
    }
    pub fn dispatch(&self,method:&Method,path:&str,auth:Option<&str>,body:&[u8])
        ->(StatusCode,Value){
        if body.len()>MAX_BODY {
            return (StatusCode::PAYLOAD_TOO_LARGE,json!({"error":"payload_too_large"}));
        }
        if method==Method::GET && path=="/healthz" {
            return (StatusCode::OK,json!({"alive":true,"publicReady":false,
                "gateway":"loopback-only","dnsConfigured":false}));
        }
        if is_account_path(path) {
            if method!=Method::POST {
                return (StatusCode::METHOD_NOT_ALLOWED,json!({"error":"method_not_allowed"}));
            }
            return match &self.accounts{
                Some(accounts)=>accounts.dispatch(path,auth,self.authorized(auth),body),
                None=>(StatusCode::SERVICE_UNAVAILABLE,json!({"error":"unavailable"})),
            };
        }
        // Operator endpoints still require the process-local admin secret.
        // Tenant endpoints instead require a scoped account session and RBAC.
        let public_action=method==Method::POST &&
            matches!(path,"/v1/agents/enroll"|"/v1/agents/proof");
        let tenant_action=method==Method::POST &&
            matches!(path,"/v1/tenant/invitations"|"/v1/tenant/routes/reserve"|
                "/v1/tenant/agents/revoke"|"/v1/tenant/agents/list"|
                "/v1/tenant/routes/list");
        if !public_action && !tenant_action && !self.authorized(auth) {
            return (StatusCode::UNAUTHORIZED,json!({"error":"unauthorized"}));
        }
        if method!=Method::POST {
            return (StatusCode::METHOD_NOT_ALLOWED,json!({"error":"method_not_allowed"}));
        }
        let mut registry=match self.registry.lock() {
            Ok(registry)=>registry,
            Err(_)=>return (StatusCode::SERVICE_UNAVAILABLE,json!({"error":"unavailable"})),
        };
        fn parse<T:serde::de::DeserializeOwned>(body:&[u8])
            ->Result<T,serde_json::Error>{
            serde_json::from_slice(body)
        }
        #[derive(Deserialize)]#[serde(rename_all="camelCase",deny_unknown_fields)]
        struct TenantInput {name:String}
        #[derive(Deserialize)]#[serde(rename_all="camelCase",deny_unknown_fields)]
        struct InvitationInput {tenant_id:String,ttl_seconds:i64}
        #[derive(Deserialize)]#[serde(rename_all="camelCase",deny_unknown_fields)]
        struct EnrollInput {invitation:String,public_key_hex:String}
        #[derive(Deserialize)]#[serde(rename_all="camelCase",deny_unknown_fields)]
        struct RouteInput {tenant_id:String,agent_id:String,site_id:String,label:String}
        #[derive(Deserialize)]#[serde(rename_all="camelCase",deny_unknown_fields)]
        struct ChallengeInput {tenant_id:String,agent_id:String,route_id:String,ttl_seconds:i64}
        #[derive(Deserialize)]#[serde(rename_all="camelCase",deny_unknown_fields)]
        struct ProofInput {
            tenant_id:String,agent_id:String,route_id:String,
            challenge:String,signature_hex:String
        }
        #[derive(Deserialize)]#[serde(rename_all="camelCase",deny_unknown_fields)]
        struct RevokeInput {tenant_id:String,agent_id:String}
        #[derive(Deserialize)]#[serde(rename_all="camelCase",deny_unknown_fields)]
        struct InventoryInput {tenant_id:String,limit:i64,offset:i64}
        let response:Result<Value,(StatusCode,&'static str)>=match path {
            "/v1/tenants"=>parse::<TenantInput>(body)
                .map_err(|_|(StatusCode::BAD_REQUEST,"invalid_json"))
                .and_then(|p| registry.create_tenant(&p.name)
                    .map(|t|json!(t)).map_err(map_network_error)),
            "/v1/invitations"=>parse::<InvitationInput>(body)
                .map_err(|_|(StatusCode::BAD_REQUEST,"invalid_json"))
                .and_then(|p|registry.issue_invitation(&p.tenant_id,p.ttl_seconds)
                    .map(|t|json!({"invitation":t.token,"expiresUnix":t.expires_unix}))
                    .map_err(map_network_error)),
            "/v1/agents/enroll"=>parse::<EnrollInput>(body)
                .map_err(|_|(StatusCode::BAD_REQUEST,"invalid_json"))
                .and_then(|p|registry.enroll_agent(&p.invitation,&p.public_key_hex)
                    .map(|a|json!(a)).map_err(map_network_error)),
            "/v1/routes/reserve"=>parse::<RouteInput>(body)
                .map_err(|_|(StatusCode::BAD_REQUEST,"invalid_json"))
                .and_then(|p|registry.reserve_hostname(&p.tenant_id,&p.agent_id,&p.site_id,&p.label)
                    .map(|r|json!({"route":r,"publiclyRoutable":false}))
                    .map_err(map_network_error)),
            "/v1/agents/challenge"=>parse::<ChallengeInput>(body)
                .map_err(|_|(StatusCode::BAD_REQUEST,"invalid_json"))
                .and_then(|p|registry.issue_route_challenge(&p.tenant_id,
                    &p.agent_id,&p.route_id,p.ttl_seconds)
                    .map(|c|json!({"challenge":c.value,"expiresUnix":c.expires_unix}))
                    .map_err(map_network_error)),
            "/v1/agents/proof"=>parse::<ProofInput>(body)
                .map_err(|_|(StatusCode::BAD_REQUEST,"invalid_json"))
                .and_then(|p|registry.verify_route_signature(&p.tenant_id,
                    &p.agent_id,&p.route_id,&p.challenge,&p.signature_hex)
                    .map(|_|json!({"identityVerified":true,"publiclyRoutable":false}))
                    .map_err(map_network_error)),
            "/v1/tenant/invitations"=>parse::<InvitationInput>(body)
                .map_err(|_|(StatusCode::BAD_REQUEST,"invalid_json"))
                .and_then(|p|{
                    let accounts=self.accounts.as_ref().ok_or(
                        (StatusCode::SERVICE_UNAVAILABLE,"unavailable"))?;
                    accounts.authorize(auth,&p.tenant_id,Capability::ManageDevices)?;
                    registry.issue_invitation(&p.tenant_id,p.ttl_seconds)
                        .map(|t|json!({"invitation":t.token,"expiresUnix":t.expires_unix}))
                        .map_err(map_network_error)
                }),
            "/v1/tenant/routes/reserve"=>parse::<RouteInput>(body)
                .map_err(|_|(StatusCode::BAD_REQUEST,"invalid_json"))
                .and_then(|p|{
                    let accounts=self.accounts.as_ref().ok_or(
                        (StatusCode::SERVICE_UNAVAILABLE,"unavailable"))?;
                    accounts.authorize(auth,&p.tenant_id,Capability::ManageSites)?;
                    registry.reserve_hostname(&p.tenant_id,&p.agent_id,&p.site_id,&p.label)
                        .map(|route|json!({"route":route,"publiclyRoutable":false}))
                        .map_err(map_network_error)
                }),
            "/v1/tenant/agents/revoke"=>parse::<RevokeInput>(body)
                .map_err(|_|(StatusCode::BAD_REQUEST,"invalid_json"))
                .and_then(|p|{
                    let accounts=self.accounts.as_ref().ok_or(
                        (StatusCode::SERVICE_UNAVAILABLE,"unavailable"))?;
                    accounts.authorize(auth,&p.tenant_id,Capability::ManageDevices)?;
                    registry.revoke_agent(&p.tenant_id,&p.agent_id)
                        .map(|_|json!({"revoked":true}))
                        .map_err(map_network_error)
                }),
            "/v1/tenant/agents/list"=>parse::<InventoryInput>(body)
                .map_err(|_|(StatusCode::BAD_REQUEST,"invalid_json"))
                .and_then(|p|{
                    let accounts=self.accounts.as_ref().ok_or(
                        (StatusCode::SERVICE_UNAVAILABLE,"unavailable"))?;
                    accounts.authorize(auth,&p.tenant_id,Capability::View)?;
                    registry.list_tenant_agents(&p.tenant_id,p.limit,p.offset)
                        .map(|items|json!({"items":items,"limit":p.limit,"offset":p.offset}))
                        .map_err(map_network_error)
                }),
            "/v1/tenant/routes/list"=>parse::<InventoryInput>(body)
                .map_err(|_|(StatusCode::BAD_REQUEST,"invalid_json"))
                .and_then(|p|{
                    let accounts=self.accounts.as_ref().ok_or(
                        (StatusCode::SERVICE_UNAVAILABLE,"unavailable"))?;
                    accounts.authorize(auth,&p.tenant_id,Capability::View)?;
                    registry.list_tenant_routes(&p.tenant_id,p.limit,p.offset)
                        .map(|items|json!({"items":items,"limit":p.limit,"offset":p.offset}))
                        .map_err(map_network_error)
                }),
            _=>Err((StatusCode::NOT_FOUND,"not_found")),
        };
        match response{
            Ok(value)=>(StatusCode::OK,value),
            Err((status,message))=>(status,json!({"error":message})),
        }
    }
}
fn map_network_error(error:NetworkError)->(StatusCode,&'static str){
    match error{
        NetworkError::Invalid|NetworkError::InvalidInvitation|
            NetworkError::InvalidChallenge=>(StatusCode::BAD_REQUEST,"invalid_request"),
        NetworkError::Unauthorized|NetworkError::Ownership=>
            (StatusCode::FORBIDDEN,"not_authorized"),
        NetworkError::Quota=>(StatusCode::CONFLICT,"quota_exceeded"),
        NetworkError::HostnameTaken=>(StatusCode::CONFLICT,"hostname_reserved"),
        NetworkError::Storage(_)=>(StatusCode::SERVICE_UNAVAILABLE,"temporarily_unavailable"),
    }
}
fn response(status:StatusCode,value:Value)->Response<Full<Bytes>>{
    let bytes=Bytes::from(value.to_string());
    Response::builder().status(status)
        .header(header::CONTENT_TYPE,"application/json; charset=utf-8")
        .header(header::CACHE_CONTROL,"no-store")
        .header(header::X_CONTENT_TYPE_OPTIONS,"nosniff")
        // No CORS grants, including Origin: null. Local browser sandboxes
        // must not receive administrator read permission.
        .body(Full::new(bytes)).expect("static response headers")
}
pub async fn serve_request(req:Request<Incoming>,controller:Arc<Controller>)
    ->Result<Response<Full<Bytes>>,std::convert::Infallible>{
    let (head,mut body)=req.into_parts();
    // Reject browser-origin requests and DNS rebinding attempts to localhost.
    // Even signed enrollment requests must not be served through a page
    // loaded from an unrelated origin.
    let host=head.headers.get(header::HOST).and_then(|v|v.to_str().ok());
    let allowed_host=host.is_some_and(|host| {
        let host=host.to_ascii_lowercase();
        host=="localhost" || host=="127.0.0.1" ||
            host.strip_prefix("localhost:").is_some_and(|p|p.parse::<u16>().is_ok()) ||
            host.strip_prefix("127.0.0.1:").is_some_and(|p|p.parse::<u16>().is_ok())
    });
    if !allowed_host || head.headers.contains_key(header::ORIGIN) {
        return Ok(response(StatusCode::FORBIDDEN,json!({"error":"local_requests_only"})));
    }
    let auth=head.headers.get(header::AUTHORIZATION)
        .and_then(|h|h.to_str().ok()).map(str::to_owned);
    let path=head.uri.path().to_owned();
    if head.headers.get(header::CONTENT_LENGTH)
        .and_then(|v|v.to_str().ok()).and_then(|s|s.parse::<usize>().ok())
        .is_some_and(|n|n>MAX_BODY){
        return Ok(response(StatusCode::PAYLOAD_TOO_LARGE,json!({"error":"payload_too_large"})));
    }
    let mut content=Vec::new();
    while let Some(frame_result)=body.frame().await{
        match frame_result{
            Ok(frame)=>{
                if let Some(chunk)=frame.data_ref(){
                    if content.len().saturating_add(chunk.len())>MAX_BODY{
                        return Ok(response(StatusCode::PAYLOAD_TOO_LARGE,
                            json!({"error":"payload_too_large"})));
                    }
                    content.extend_from_slice(chunk);
                }
            }
            Err(_)=>return Ok(response(StatusCode::BAD_REQUEST,json!({"error":"invalid_body"}))),
        }
    }
    let (status,value)=controller.dispatch(&head.method,&path,auth.as_deref(),&content);
    Ok(response(status,value))
}
#[cfg(test)]
mod tests{
    use super::*;
    fn api()->Controller{
        Controller::new(Registry::memory("example.test").unwrap(),
            b"0123456789abcdef0123456789abcdef".to_vec()).unwrap()
    }
    fn command(api:&Controller,path:&str,admin:bool,input:Value)->(StatusCode,Value){
        api.dispatch(&Method::POST,path,
            admin.then_some("Bearer 0123456789abcdef0123456789abcdef"),
            input.to_string().as_bytes())
    }
    #[test]
    fn rejects_admin_operations_without_authorization_and_never_mints_routes(){
        let api=api();
        let (status,_)=command(&api,"/v1/tenants",false,json!({"name":"Tenant"}));
        assert_eq!(status,StatusCode::UNAUTHORIZED);
        let (status,tenant)=command(&api,"/v1/tenants",true,json!({"name":"Tenant"}));
        assert_eq!(status,StatusCode::OK);
        let id=tenant["id"].as_str().unwrap();
        let (status,invitation)=command(&api,"/v1/invitations",true,
            json!({"tenantId":id,"ttlSeconds":120}));
        assert_eq!(status,StatusCode::OK);
        let (status,agent)=command(&api,"/v1/agents/enroll",false,
            json!({"invitation":invitation["invitation"],"publicKeyHex":"42".repeat(32)}));
        assert_eq!(status,StatusCode::OK);
        let (status,route)=command(&api,"/v1/routes/reserve",true,
            json!({"tenantId":id,"agentId":agent["id"],
                "siteId":Uuid::new_v4().to_string(),"label":"first-host"}));
        assert_eq!(status,StatusCode::OK);
        assert_eq!(route["publiclyRoutable"],false);
        let (status,_) = command(&api,"/v1/tenants",false,json!({"name":"NotAllowed"}));
        assert_eq!(status,StatusCode::UNAUTHORIZED);
    }
    #[tokio::test]
    async fn real_http_listener_is_loopback_only_and_has_no_public_readiness(){
        use hyper::{service::service_fn,server::conn::http1};
        use hyper_util::rt::TokioIo;
        use tokio::{net::{TcpListener,TcpStream},io::{AsyncReadExt,AsyncWriteExt}};
        let listener=TcpListener::bind(("127.0.0.1",0)).await.unwrap();
        let port=listener.local_addr().unwrap().port();
        let control=Arc::new(api());
        let task=tokio::spawn(async move{
            let (socket,_)=listener.accept().await.unwrap();
            let service=service_fn(move |request|serve_request(request,control.clone()));
            http1::Builder::new().serve_connection(TokioIo::new(socket),service)
                .await.unwrap();
        });
        let mut client=TcpStream::connect(("127.0.0.1",port)).await.unwrap();
        client.write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await.unwrap();
        let mut response=Vec::new();
        tokio::time::timeout(std::time::Duration::from_secs(5),
            client.read_to_end(&mut response)).await.unwrap().unwrap();
        let text=String::from_utf8(response).unwrap();
        assert!(text.starts_with("HTTP/1.1 200"),"{text}");
        assert!(text.contains("\"publicReady\":false"),"{text}");
        assert!(!text.contains("access-control-allow-origin:"));
        task.await.unwrap();
    }
    #[tokio::test]
    async fn http_server_rejects_foreign_host_and_browser_origin() {
        use hyper::{service::service_fn,server::conn::http1};
        use hyper_util::rt::TokioIo;
        use tokio::{net::{TcpListener,TcpStream},io::{AsyncReadExt,AsyncWriteExt}};
        for headers in ["Host: evil.example.test\r\n",
                        "Host: localhost\r\nOrigin: https://evil.example.test\r\n"] {
            let listener=TcpListener::bind(("127.0.0.1",0)).await.unwrap();
            let port=listener.local_addr().unwrap().port();
            let control=Arc::new(api());
            let server=tokio::spawn(async move {
                let (stream,_)=listener.accept().await.unwrap();
                let service=service_fn(move |request|serve_request(request,control.clone()));
                http1::Builder::new().serve_connection(TokioIo::new(stream),service).await.unwrap();
            });
            let mut client=TcpStream::connect(("127.0.0.1",port)).await.unwrap();
            let request=format!("GET /healthz HTTP/1.1\r\n{headers}Connection: close\r\n\r\n");
            client.write_all(request.as_bytes()).await.unwrap();
            let mut response=Vec::new();
            tokio::time::timeout(std::time::Duration::from_secs(5),
                client.read_to_end(&mut response)).await.unwrap().unwrap();
            assert!(String::from_utf8_lossy(&response).starts_with("HTTP/1.1 403"));
            server.await.unwrap();
        }
    }
    #[test]
    fn local_liveness_is_not_public_readiness_and_invalid_json_fails_closed(){
        let api=api();
        let (s,health)=api.dispatch(&Method::GET,"/healthz",None,&[]);
        assert_eq!(s,StatusCode::OK);
        assert_eq!(health["publicReady"],false);
        let (s,_) = api.dispatch(&Method::POST,"/v1/tenants",
            Some("Bearer incorrect"),b"{\"name\":\"a\"}");
        assert_eq!(s,StatusCode::UNAUTHORIZED);
        let (s,_) = api.dispatch(&Method::POST,"/v1/tenants",
            Some("Bearer 0123456789abcdef0123456789abcdef"),b"{oops");
        assert_eq!(s,StatusCode::BAD_REQUEST);
        let (s,_) = api.dispatch(&Method::POST,"/v1/tenants",
            Some("Bearer 0123456789abcdef0123456789abcdef"),
            &vec![b'X';MAX_BODY+1]);
        assert_eq!(s,StatusCode::PAYLOAD_TOO_LARGE);
    }
    #[test]
    fn localhost_account_sessions_enforce_tenant_roles_and_revoke_tokens(){
        let path=std::env::temp_dir().join(format!("hostify-tenant-http-{}.db",Uuid::new_v4()));
        let mut registry=Registry::open(&path,"example.test").unwrap();
        let first=registry.create_tenant("First").unwrap();
        let second=registry.create_tenant("Second").unwrap();
        let accounts=Accounts::open(&path).unwrap();
        let api=Controller::new(registry,
            b"0123456789abcdef0123456789abcdef".to_vec()).unwrap().with_accounts(accounts);
        let (status,_) = command(&api,"/v1/accounts/bootstrap",false,json!({
            "tenantId":first.id,"email":"owner@site.test","password":"valid-owner-password"
        }));
        assert_eq!(status,StatusCode::UNAUTHORIZED);
        let (status,created) = command(&api,"/v1/accounts/bootstrap",true,json!({
            "tenantId":first.id,"email":"owner@site.test","password":"valid-owner-password"
        }));
        assert_eq!(status,StatusCode::OK);
        assert_eq!(created["owner"]["tenantId"],first.id);
        let (status,_) = command(&api,"/v1/accounts/bootstrap",true,json!({
            "tenantId":first.id,"email":"other@site.test","password":"valid-other-password"
        }));
        assert_eq!(status,StatusCode::FORBIDDEN);
        let (status,logged) = command(&api,"/v1/auth/login",false,json!({
            "tenantId":first.id,"email":"owner@site.test","password":"valid-owner-password"
        }));
        assert_eq!(status,StatusCode::OK);
        let token=logged["token"].as_str().unwrap().to_owned();
        assert_eq!(token.len(),64);
        let owner_auth=format!("Bearer {token}");
        let call=|path:&str,auth:&str,input:Value|{
            api.dispatch(&Method::POST,path,Some(auth),input.to_string().as_bytes())
        };
        let (status,me)=call("/v1/auth/me",&owner_auth,json!({"tenantId":first.id}));
        assert_eq!(status,StatusCode::OK);
        assert_eq!(me["user"]["role"],"owner");
        let (status,_) = call("/v1/auth/me",&owner_auth,json!({"tenantId":second.id}));
        assert_eq!(status,StatusCode::UNAUTHORIZED);
        let (status,invite)=call("/v1/members/invite",&owner_auth,json!({
            "tenantId":first.id,"email":"viewer@site.test","role":"viewer","ttlSeconds":600
        }));
        assert_eq!(status,StatusCode::OK);
        let (status,member)=command(&api,"/v1/members/accept",false,json!({
            "invitation":invite["invitation"],"password":"valid-viewer-password"
        }));
        assert_eq!(status,StatusCode::OK);
        assert_eq!(member["member"]["role"],"viewer");
        let (status,viewer)=command(&api,"/v1/auth/login",false,json!({
            "tenantId":first.id,"email":"viewer@site.test","password":"valid-viewer-password"
        }));
        assert_eq!(status,StatusCode::OK);
        let viewer_auth=format!("Bearer {}",viewer["token"].as_str().unwrap());
        let (status,_)=call("/v1/tenant/invitations",&viewer_auth,json!({
            "tenantId":first.id,"ttlSeconds":120
        }));
        assert_eq!(status,StatusCode::FORBIDDEN);
        let (status,device_invite)=call("/v1/tenant/invitations",&owner_auth,json!({
            "tenantId":first.id,"ttlSeconds":120
        }));
        assert_eq!(status,StatusCode::OK);
        let (status,agent)=command(&api,"/v1/agents/enroll",false,json!({
            "invitation":device_invite["invitation"],"publicKeyHex":"11".repeat(32)
        }));
        assert_eq!(status,StatusCode::OK);
        let route=json!({"tenantId":first.id,"agentId":agent["id"],
            "siteId":Uuid::new_v4().to_string(),"label":"team-home"});
        let (status,_) = call("/v1/tenant/routes/reserve",&viewer_auth,route.clone());
        assert_eq!(status,StatusCode::FORBIDDEN);
        let (status,_) = call("/v1/tenant/routes/reserve",&owner_auth,json!({
            "tenantId":second.id,"agentId":agent["id"],
            "siteId":Uuid::new_v4().to_string(),"label":"foreign-home"
        }));
        assert_eq!(status,StatusCode::UNAUTHORIZED);
        let (status,reserved)=call("/v1/tenant/routes/reserve",&owner_auth,route);
        assert_eq!(status,StatusCode::OK);
        assert_eq!(reserved["publiclyRoutable"],false);
        assert_eq!(reserved["route"]["hostname"],"team-home.example.test");
        let query=json!({"tenantId":first.id,"limit":10,"offset":0});
        let (status,devices)=call("/v1/tenant/agents/list",&viewer_auth,query.clone());
        assert_eq!(status,StatusCode::OK);
        assert_eq!(devices["items"].as_array().unwrap().len(),1);
        assert_eq!(devices["items"][0]["id"],agent["id"]);
        let (status,routes)=call("/v1/tenant/routes/list",&viewer_auth,query.clone());
        assert_eq!(status,StatusCode::OK);
        assert_eq!(routes["items"].as_array().unwrap().len(),1);
        assert_eq!(routes["items"][0]["hostname"],"team-home.example.test");
        let (status,page_two)=call("/v1/tenant/routes/list",&viewer_auth,json!({
            "tenantId":first.id,"limit":1,"offset":1
        }));
        assert_eq!(status,StatusCode::OK);
        assert_eq!(page_two["items"].as_array().unwrap().len(),0);
        let (status,_) = call("/v1/tenant/routes/list",&viewer_auth,json!({
            "tenantId":second.id,"limit":10,"offset":0
        }));
        assert_eq!(status,StatusCode::UNAUTHORIZED);
        let (status,_) = call("/v1/tenant/agents/list",&viewer_auth,json!({
            "tenantId":first.id,"limit":101,"offset":0
        }));
        assert_eq!(status,StatusCode::BAD_REQUEST);
        let (status,revoke)=call("/v1/tenant/agents/revoke",&owner_auth,json!({
            "tenantId":first.id,"agentId":agent["id"]
        }));
        assert_eq!(status,StatusCode::OK);
        assert_eq!(revoke["revoked"],true);
        let (status,remaining)=call("/v1/tenant/routes/list",&viewer_auth,query.clone());
        assert_eq!(status,StatusCode::OK);
        assert!(remaining["items"].as_array().unwrap().is_empty());
        let (status,remaining)=call("/v1/tenant/agents/list",&viewer_auth,query);
        assert_eq!(status,StatusCode::OK);
        assert!(remaining["items"].as_array().unwrap().is_empty());
        let (status,_)=call("/v1/auth/logout",&owner_auth,json!({}));
        assert_eq!(status,StatusCode::OK);
        let (status,_)=call("/v1/auth/me",&owner_auth,json!({"tenantId":first.id}));
        assert_eq!(status,StatusCode::UNAUTHORIZED);
        drop(api);
        std::fs::remove_file(&path).unwrap();
    }

}
