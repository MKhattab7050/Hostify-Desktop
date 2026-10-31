//! Hostify Network control plane: localhost-only administration and agent
//! enrollment gateway. No public networking, DNS changes or workload execution.
use bytes::Bytes;
use hostify_core::network::{NetworkError,Registry};
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
}
impl Controller {
    pub fn new(registry:Registry,admin_secret:Vec<u8>)->Result<Self,&'static str>{
        if admin_secret.len()<32 || admin_secret.len()>4096 ||
            admin_secret.iter().any(|b| !b.is_ascii_graphic()) {
            return Err("Configure a random administrator secret of 32-4096 printable ASCII bytes.");
        }
        Ok(Self{registry:Mutex::new(registry),admin_secret})
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
        // Only enrollment and proof-of-possession endpoints accept a one-use
        // token without administrator credentials. They are still loopback-
        // only until a real HTTPS reverse proxy and abuse controls exist.
        let public_action=method==Method::POST &&
            matches!(path,"/v1/agents/enroll"|"/v1/agents/proof");
        if !public_action && !self.authorized(auth) {
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
}
