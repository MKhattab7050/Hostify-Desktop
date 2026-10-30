use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{time::{Duration, Instant}, sync::{Arc, Mutex}};
pub type TunnelConfig = Value;
type ApiLogger = Arc<dyn Fn(&str) + Send + Sync>;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CfError {
    #[error("Cloudflare resource not found")] NotFound,
    #[error("Invalid Cloudflare token")] InvalidToken,
    #[error("Missing Cloudflare permission: {0}")] MissingPermission(String),
    #[error("Cloudflare zone not found")] ZoneNotFound,
    #[error("DNS record conflict")] Conflict,
    #[error("Cloudflare rate limit reached")] RateLimited,
    #[error("Cloudflare network error: {0}")] Network(String),
    #[error("{}", api_message(*status, *code, message))] Api { status: u16, code: i64, message: String },
    #[error("Unexpected Cloudflare response: {0}")] Unexpected(String),
}
impl CfError {
 pub fn is_not_found(&self)->bool { matches!(self, Self::NotFound | Self::Api{status:404,..}) }
 pub fn friendly(&self) -> String { self.to_string() }

}
fn api_message(status:u16, code:i64, message:&str)->String {
    let category=match (status,code) {
        (401,_)|(_,9109|10000)=>"Invalid Cloudflare token",
        (403,_)|(_,9103)=>"Missing Cloudflare permission",
        (429,_)|(_,1015)=>"Cloudflare rate limit reached",
        (404,_)=>"Cloudflare resource not found",
        _=>"Cloudflare API error",
    };
    format!("{category} (HTTP {status}, code {code}): {message}")
}
#[derive(Clone)]
pub struct CloudflareClient { base: String, token: String, client: reqwest::Client, logger: Option<ApiLogger>, secrets: Arc<Mutex<Vec<String>>> }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all="camelCase")]
pub struct Account { pub id: String, pub name: String }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all="camelCase")]
pub struct Zone { pub id: String, pub name: String }
#[derive(Clone)]
pub struct Tunnel { pub id: String, pub token: String, pub created: bool }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExistingRecord { #[serde(rename="type")] pub kind: String, pub content: String }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all="camelCase")]
pub struct DnsPlan { pub hostname: String, pub action: String, #[serde(skip_serializing_if="Option::is_none")] pub existing: Option<ExistingRecord>, pub message: String }
impl CloudflareClient {
    /// Split operations so deploy can journal the remote ID before fetching credentials.
    pub async fn find_tunnel(&self, account: &str, name: &str) -> Result<Option<String>, CfError> {
        let items = self.pages(&format!("/accounts/{account}/cfd_tunnel")).await?;
        Ok(items.iter().find(|v| v["name"].as_str() == Some(name) && v.get("deleted_at").is_none_or(Value::is_null)).and_then(|v| v["id"].as_str()).map(str::to_owned))
    }
    pub async fn create_tunnel(&self, account: &str, name: &str) -> Result<String, CfError> {
        let v = self.request(reqwest::Method::POST, &format!("/accounts/{account}/cfd_tunnel"), Some(json!({"name":name,"config_src":"cloudflare"}))).await?;
        v["result"]["id"].as_str().map(str::to_owned).ok_or_else(|| CfError::Unexpected("Missing tunnel ID".into()))
    }
    pub async fn tunnel_token(&self, account: &str, id: &str) -> Result<String, CfError> {
        let v = self.request(reqwest::Method::GET, &format!("/accounts/{account}/cfd_tunnel/{id}/token"), None).await?;
        v["result"].as_str().map(str::to_owned).ok_or_else(|| CfError::Unexpected("Missing tunnel token".into()))
    }
    pub async fn get_ingress(&self, account: &str, id: &str) -> Result<Option<TunnelConfig>, CfError> {
        let v = match self.request(reqwest::Method::GET, &format!("/accounts/{account}/cfd_tunnel/{id}/configurations"), None).await {
            Ok(v)=>v, Err(e) if e.is_not_found()=>return Ok(None), Err(e)=>return Err(e)
        };
        let result=v.get("result").ok_or_else(||CfError::Unexpected("Missing configuration result".into()))?;
        if !result.is_null() && !result.is_object() {return Err(CfError::Unexpected("Expected configuration result object".into()));}
        let config=&result["config"];
        if config.is_null() { return Ok(None); }
        if !config.is_object() { return Err(CfError::Unexpected("Expected config object".into())); }
        match config.get("ingress") { None | Some(Value::Null)=>Ok(None), Some(Value::Array(_))=>Ok(Some(config.clone())), _=>Err(CfError::Unexpected("Expected ingress array".into())) }
    }
    pub async fn put_config(&self, account: &str, id: &str, config: Value) -> Result<(), CfError> {
        self.request(reqwest::Method::PUT, &format!("/accounts/{account}/cfd_tunnel/{id}/configurations"), Some(json!({"config":config}))).await?; Ok(())
    }
    pub async fn dns_preflight(&self, zone: &str, hostname: &str) -> Result<(), CfError> {
        for v in self.records(zone, hostname).await? {
            if ["A","AAAA","CNAME","TXT"].contains(&v["type"].as_str().unwrap_or("")) && (v["type"] != "CNAME" || v["comment"] != "managed-by=hostify") { return Err(CfError::Conflict); }
        }
        Ok(())
    }
    pub async fn find_deployment_dns(&self, zone: &str, hostname: &str, owner: &str) -> Result<Option<String>, CfError> {
        Ok(self.records(zone, hostname).await?.iter().find(|v| v["comment"] == "managed-by=hostify" && v["tags"].as_array().is_some_and(|tags| tags.contains(&json!(format!("hostify-deployment:{owner}"))))).and_then(|v| v["id"].as_str()).map(str::to_owned))
    }
    pub async fn create_deployment_dns(&self, zone: &str, hostname: &str, tunnel: &str, owner: &str) -> Result<String, CfError> {
        if self.plan_dns(zone, hostname, tunnel).await?.action != "create" { return Err(CfError::Conflict); }
        let v = self.request(reqwest::Method::POST, &format!("/zones/{zone}/dns_records"), Some(json!({"type":"CNAME","name":hostname,"content":format!("{tunnel}.cfargotunnel.com"),"proxied":true,"comment":"managed-by=hostify","tags":[format!("hostify-deployment:{owner}")]}))).await?;
        { let id=v["result"]["id"].as_str().ok_or_else(||CfError::Unexpected("Missing DNS record ID".into()))?; Ok(id.to_owned()) }
    }
    pub fn new(base_url: impl AsRef<str>, token: impl Into<String>) -> Self { Self { base: base_url.as_ref().trim_end_matches('/').to_owned(), token: token.into(), logger: None, secrets: Arc::new(Mutex::new(Vec::new())), client: reqwest::Client::builder().timeout(Duration::from_secs(20)).build().unwrap_or_default() } }
    pub fn production(token: impl Into<String>) -> Self { Self::new("https://api.cloudflare.com/client/v4", token) }
    pub fn with_logger(mut self, logger: impl Fn(&str)+Send+Sync+'static)->Self { self.logger=Some(Arc::new(logger)); self }
    async fn request(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value, CfError> {
        let start=Instant::now();
        let mut req=self.client.request(method.clone(),format!("{}{path}",self.base)).bearer_auth(&self.token);
        if let Some(v)=body {req=req.json(&v);}
        let response=match req.send().await {Ok(r)=>r,Err(_)=>{
            self.log(&method,path,0,"transport failure",start); return Err(CfError::Network("Could not reach Cloudflare".into()));
        }};
        let status=response.status().as_u16();
        let bytes=match response.bytes().await {Ok(bytes)=>bytes,Err(_)=>{self.log(&method,path,status,"response transport failure",start);return Err(CfError::Network("Could not read Cloudflare response".into()));}};
        let parsed=serde_json::from_slice::<Value>(&bytes);
        let errors=parsed.as_ref().ok().and_then(|v|v["errors"].as_array()).cloned().unwrap_or_default();
        // Register token responses before logging; no result or body is ever logged.
        if path.ends_with("/token") {if let Some(token)=parsed.as_ref().ok().and_then(|v|v["result"].as_str()) {self.secrets.lock().unwrap().push(token.to_owned());}}
        let detail=errors.iter().map(|e|format!("{}: {}",e["code"],e["message"].as_str().unwrap_or("API error"))).collect::<Vec<_>>().join("; ");
        self.log(&method,path,status,&detail,start);
        if !(200..300).contains(&status) || parsed.as_ref().ok().is_some_and(|v|v["success"]==false) {
            let code=errors.first().and_then(|e|e["code"].as_i64()).unwrap_or(0);
            let message=errors.first().and_then(|e|e["message"].as_str()).unwrap_or(match status {401=>"Invalid token",403=>"Access denied",429=>"Rate limit reached",404=>"Resource not found",_=>"Cloudflare API request failed"});
            return Err(CfError::Api{status,code,message:self.redact(message)});
        }
        let v=parsed.map_err(|_|CfError::Unexpected("Expected JSON response".into()))?;
        if v["success"]!=true {return Err(CfError::Unexpected("Missing success flag".into()));}
        Ok(v)
    }
    fn redact(&self, text:&str)->String {
        let mut out=if self.token.is_empty(){text.to_owned()}else{text.replace(&self.token,"[redacted]")};
        for secret in self.secrets.lock().unwrap().iter() {if !secret.is_empty(){out=out.replace(secret,"[redacted]");}}
        out.replace(['\r','\n']," ")
    }
    fn log(&self, method:&reqwest::Method,path:&str,status:u16,detail:&str,start:Instant) {
        if let Some(logger)=&self.logger {logger(&format!("{} {} {} status={} errors=[{}] duration_ms={}",chrono::Utc::now().to_rfc3339(),method,self.redact(path),status,self.redact(detail),start.elapsed().as_millis()));}
    }
    async fn pages(&self, path: &str) -> Result<Vec<Value>, CfError> {
        let mut page = 1; let mut all = Vec::new();
        loop { let sep = if path.contains('?') { '&' } else { '?' }; let v = self.request(reqwest::Method::GET, &format!("{path}{sep}per_page=100&page={page}"), None).await?;
            let items = v.get("result").and_then(Value::as_array).ok_or_else(|| CfError::Unexpected("Missing result array in list response".into()))?; all.extend(items.iter().cloned());
            let total = v.pointer("/result_info/total_pages").and_then(Value::as_u64).unwrap_or(1); if page >= total { break; } page += 1;
        } Ok(all)
    }
    pub async fn verify_token(&self) -> Result<(), CfError> { let v=self.request(reqwest::Method::GET, "/user/tokens/verify", None).await?; match v.pointer("/result/status").and_then(Value::as_str) {Some("active")=>Ok(()),Some(_)=>Err(CfError::InvalidToken),None=>Err(CfError::Unexpected("Missing token verification status".into()))} }
    pub async fn list_accounts(&self) -> Result<Vec<Account>, CfError> { self.pages("/accounts").await?.into_iter().map(|v| serde_json::from_value(v).map_err(|e| CfError::Unexpected(e.to_string()))).collect() }
    pub async fn find_zone(&self, hostname: &str) -> Result<Zone, CfError> {
        let labels: Vec<_> = hostname.trim_end_matches('.').split('.').collect();
        for i in 0..labels.len().saturating_sub(1) { let name=labels[i..].join("."); let url = reqwest::Url::parse_with_params("http://localhost/", &[("name", &name)]).map_err(|e| CfError::Unexpected(e.to_string()))?; let path = format!("/zones?{}", url.query().unwrap_or(""));
            for v in self.pages(&path).await? { let z: Zone=serde_json::from_value(v).map_err(|e| CfError::Unexpected(e.to_string()))?; if z.name.eq_ignore_ascii_case(&name) { return Ok(z); } }
        } Err(CfError::ZoneNotFound)
    }
    pub async fn check_dns_permission(&self, zone: &str) -> Result<(), CfError> { self.pages(&format!("/zones/{zone}/dns_records")).await.map(|_| ()) }
    pub async fn get_or_create_tunnel(&self, account: &str, name: &str) -> Result<Tunnel, CfError> {
        let (id,created)=match self.find_tunnel(account,name).await? {Some(id)=>(id,false),None=>(self.create_tunnel(account,name).await?,true)};
        let token=self.tunnel_token(account,&id).await?;
        Ok(Tunnel{id,token,created})
    }
    pub async fn delete_tunnel(&self, account: &str, id: &str) -> Result<(), CfError> {
        let path=format!("/accounts/{account}/cfd_tunnel/{id}");
        match self.request(reqwest::Method::DELETE,&path,None).await {
            Ok(_)=>Ok(()), Err(e) if e.is_not_found()=>Ok(()),
            Err(CfError::Api{code:1022,..}) | Err(CfError::Api{status:409,..})=>{
                match self.request(reqwest::Method::DELETE,&format!("{path}/connections"),None).await {Ok(_)=>(),Err(e) if e.is_not_found()=>(),Err(e)=>return Err(e)};
                match self.request(reqwest::Method::DELETE,&path,None).await {Ok(_)=>Ok(()),Err(e) if e.is_not_found()=>Ok(()),Err(e)=>Err(e)}
            }, Err(e)=>Err(e)
        }
    }
    pub async fn put_tunnel_ingress(&self, account: &str, id: &str, entries: &[(String,String)]) -> Result<(), CfError> { let mut ingress: Vec<Value>=entries.iter().map(|(hostname,service)| json!({"hostname":hostname,"service":service})).collect(); ingress.push(json!({"service":"http_status:404"})); self.request(reqwest::Method::PUT,&format!("/accounts/{account}/cfd_tunnel/{id}/configurations"),Some(json!({"config":{"ingress":ingress}}))).await.map(|_| ()) }
    async fn records(&self, zone: &str, hostname: &str) -> Result<Vec<Value>, CfError> { let url=reqwest::Url::parse_with_params("http://localhost/", &[("name",hostname)]).map_err(|e| CfError::Unexpected(e.to_string()))?; self.pages(&format!("/zones/{zone}/dns_records?{}",url.query().unwrap_or(""))).await }
    pub async fn plan_dns(&self, zone: &str, hostname: &str, tunnel: &str) -> Result<DnsPlan, CfError> {
        let target=format!("{tunnel}.cfargotunnel.com"); let mut own=None;
        for v in self.records(zone,hostname).await? { let kind=v["type"].as_str().unwrap_or(""); if !["A","AAAA","CNAME","TXT"].contains(&kind) { continue; } let content=v["content"].as_str().unwrap_or(""); let existing=Some(ExistingRecord{kind:kind.into(),content:content.into()}); if kind=="CNAME" && v["comment"].as_str()==Some("managed-by=hostify") { own=existing; } else { return Ok(DnsPlan{hostname:hostname.into(),action:"conflict".into(),existing,message:"An existing DNS record conflicts with this hostname".into()}); } }
        Ok(DnsPlan{hostname:hostname.into(),action:if own.is_some(){"update_own"}else{"create"}.into(),message:if own.as_ref().is_some_and(|e|e.content.eq_ignore_ascii_case(&target)){"Already points at this tunnel; reuse without changes"}else{"DNS record can be configured"}.into(),existing:own})
    }
    pub async fn create_dns(&self, zone: &str, hostname: &str, tunnel: &str) -> Result<String, CfError> {
        let plan=self.plan_dns(zone,hostname,tunnel).await?;
        if plan.action=="update_own" && plan.existing.as_ref().is_some_and(|e|e.content.eq_ignore_ascii_case(&format!("{tunnel}.cfargotunnel.com"))) {
            return self.dns_record(zone,hostname).await?.and_then(|v|v["id"].as_str().map(str::to_owned)).ok_or_else(||CfError::Unexpected("Missing reusable DNS record ID".into()));
        }
        if plan.action != "create" {return Err(CfError::Conflict);}
 let v=self.request(reqwest::Method::POST,&format!("/zones/{zone}/dns_records"),Some(json!({"type":"CNAME","name":hostname,"content":format!("{tunnel}.cfargotunnel.com"),"proxied":true,"comment":"managed-by=hostify"}))).await?; { let id=v["result"]["id"].as_str().ok_or_else(||CfError::Unexpected("Missing DNS record ID".into()))?; self.verify_dns(zone,id,tunnel).await?; Ok(id.to_owned()) } }
    pub async fn dns_record(&self,zone:&str,hostname:&str)->Result<Option<Value>,CfError> {
        Ok(self.records(zone,hostname).await?.into_iter().find(|v|v["type"]=="CNAME" && v["comment"]=="managed-by=hostify"))
    }
    pub async fn write_dns(&self,zone:&str,id:&str,record:Value)->Result<(),CfError> {
        let mut body=serde_json::Map::new();
        for key in ["type","name","content","proxied","ttl","comment","tags","settings"] {if let Some(value)=record.get(key) {body.insert(key.into(),value.clone());}}
        self.request(reqwest::Method::PUT,&format!("/zones/{zone}/dns_records/{id}"),Some(Value::Object(body))).await?; Ok(())
    }
    pub async fn verify_dns(&self,zone:&str,id:&str,tunnel:&str)->Result<(),CfError> {
        let v=self.request(reqwest::Method::GET,&format!("/zones/{zone}/dns_records/{id}"),None).await?;
        if v["result"]["content"]!=format!("{tunnel}.cfargotunnel.com") || v["result"]["proxied"]!=true {return Err(CfError::Unexpected("DNS readback must match tunnel target and proxied=true".into()));} Ok(())
    }
    pub async fn delete_owned_dns(&self,zone:&str,id:&str,owner:&str)->Result<bool,CfError> {
        let path=format!("/zones/{zone}/dns_records/{id}");
        let v=match self.request(reqwest::Method::GET,&path,None).await {Ok(v)=>v,Err(e) if e.is_not_found()=>return Ok(false),Err(e)=>return Err(e)};
        if v["result"]["comment"]!="managed-by=hostify" || !v["result"]["tags"].as_array().is_some_and(|tags|tags.contains(&json!(format!("hostify-deployment:{owner}")))) {return Ok(false);}
        match self.request(reqwest::Method::DELETE,&path,None).await {Ok(_)=>Ok(true),Err(e) if e.is_not_found()=>Ok(false),Err(e)=>Err(e)}
    }
    pub async fn delete_dns_if_ours(&self, zone: &str, id: &str) -> Result<bool, CfError> { let path=format!("/zones/{zone}/dns_records/{id}"); let v=match self.request(reqwest::Method::GET,&path,None).await { Ok(v)=>v, Err(e) if e.is_not_found()=>return Ok(false), Err(e)=>return Err(e) }; if v["result"]["comment"].as_str()!=Some("managed-by=hostify") { return Ok(false); } match self.request(reqwest::Method::DELETE,&path,None).await {Ok(_)=>Ok(true),Err(e) if e.is_not_found()=>Ok(false),Err(e)=>Err(e)} }
}
#[cfg(test)] mod tests {
    use super::*;
    use wiremock::{MockServer, Mock, ResponseTemplate, matchers::{method,path,query_param}};
    #[tokio::test] async fn token_and_permission_errors() {
        let s=MockServer::start().await;
        Mock::given(method("GET")).and(path("/user/tokens/verify")).respond_with(ResponseTemplate::new(401)).mount(&s).await;
        Mock::given(method("GET")).and(path("/zones/z/dns_records")).respond_with(ResponseTemplate::new(403)).mount(&s).await;
        let c=CloudflareClient::new(s.uri(),"secret"); assert!(matches!(c.verify_token().await,Err(CfError::Api{status:401,..}))); assert!(matches!(c.check_dns_permission("z").await,Err(CfError::Api{status:403,..})));
    }
    #[tokio::test] async fn walks_zone_labels() {
        let s=MockServer::start().await;
        for name in ["a.b.example.com","b.example.com"] { Mock::given(path("/zones")).and(query_param("name",name)).respond_with(ResponseTemplate::new(200).set_body_json(json!({"success":true,"result":[]}))).expect(1).mount(&s).await; }
        Mock::given(path("/zones")).and(query_param("name","example.com")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"success":true,"result":[{"id":"z","name":"example.com"}]}))).expect(1).mount(&s).await;
        assert_eq!(CloudflareClient::new(s.uri(),"x").find_zone("a.b.example.com").await.expect("zone").id,"z");
    }
    #[tokio::test] async fn existing_tunnel_is_reused() {
        let s=MockServer::start().await;
        Mock::given(method("GET")).and(path("/accounts/a/cfd_tunnel")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"success":true,"result":[{"id":"t","name":"site"}]}))).mount(&s).await;
        Mock::given(method("GET")).and(path("/accounts/a/cfd_tunnel/t/token")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"success":true,"result":"token"}))).mount(&s).await;
        let t=CloudflareClient::new(s.uri(),"x").get_or_create_tunnel("a","site").await.expect("tunnel"); assert!(!t.created); assert_eq!(t.id,"t");
    }
    #[tokio::test] async fn conflict_and_delete_only_tagged_records() {
        let s=MockServer::start().await;
        Mock::given(method("GET")).and(path("/zones/z/dns_records")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"success":true,"result":[{"type":"A","content":"192.0.2.1"}]}))).mount(&s).await;
        Mock::given(method("GET")).and(path("/zones/z/dns_records/other")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"success":true,"result":{"comment":"other"}}))).mount(&s).await;
        Mock::given(method("GET")).and(path("/zones/z/dns_records/ours")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"success":true,"result":{"comment":"managed-by=hostify"}}))).mount(&s).await;
        Mock::given(method("DELETE")).and(path("/zones/z/dns_records/ours")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"success":true,"result":{}}))).expect(1).mount(&s).await;
        let c=CloudflareClient::new(s.uri(),"x"); assert_eq!(c.plan_dns("z","www.example.com","t").await.expect("plan").action,"conflict"); assert!(!c.delete_dns_if_ours("z","other").await.expect("other")); assert!(c.delete_dns_if_ours("z","ours").await.expect("ours"));
    }
}

#[cfg(test)] mod dns_create_test {
 use super::*; use wiremock::{Mock, MockServer, ResponseTemplate, matchers::{method,path,body_partial_json}};
 #[tokio::test] async fn creates_tagged_proxied_cname() { let s=MockServer::start().await;
 Mock::given(method("GET")).and(path("/zones/z/dns_records")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"success":true,"result":[]}))).mount(&s).await;
 Mock::given(method("POST")).and(path("/zones/z/dns_records")).and(body_partial_json(json!({"type":"CNAME","name":"www.example.com","content":"t.cfargotunnel.com","proxied":true,"comment":"managed-by=hostify"}))).respond_with(ResponseTemplate::new(200).set_body_json(json!({"success":true,"result":{"id":"r"}}))).expect(1).mount(&s).await;
 Mock::given(method("GET")).and(path("/zones/z/dns_records/r")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"success":true,"result":{"content":"t.cfargotunnel.com","proxied":true}}))).mount(&s).await;
 assert_eq!(CloudflareClient::new(s.uri(),"token").create_dns("z","www.example.com","t").await.expect("create"),"r"); }
}

#[cfg(test)] mod regression_tests {
 use super::*;
 use wiremock::{Mock,MockServer,ResponseTemplate,matchers::{method,path,query_param}};
 #[tokio::test] async fn unconfigured_variants_and_not_found_are_none_but_bad_shapes_are_unexpected() {
  for result in [json!({"config":null}),json!({"config":{}}),json!({}),json!({"config":{"originRequest":{}}})] {
   let s=MockServer::start().await;Mock::given(path("/accounts/a/cfd_tunnel/t/configurations")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"success":true,"result":result}))).mount(&s).await;
   assert!(CloudflareClient::new(s.uri(),"api-secret").get_ingress("a","t").await.unwrap().is_none());
  }
  for status in [401,403,429,404,500] {
   let s=MockServer::start().await;Mock::given(method("GET")).respond_with(ResponseTemplate::new(status).set_body_json(json!({"success":false,"errors":[{"code":123,"message":"provider explanation"}]}))).mount(&s).await;
   let c=CloudflareClient::new(s.uri(),"api-secret");
   assert!(matches!(c.tunnel_token("a","t").await,Err(CfError::Api{status:s,code:123,message}) if s==status && message=="provider explanation"));
   if status==404 {assert!(c.get_ingress("a","t").await.unwrap().is_none());}
  }
  for body in [json!({"success":true}),json!({"success":true,"result":{"config":{"ingress":"wrong"}}}),json!({"success":true,"result":{"config":[]}})] {
   let s=MockServer::start().await;Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(body)).mount(&s).await;
   assert!(matches!(CloudflareClient::new(s.uri(),"api-secret").get_ingress("a","t").await,Err(CfError::Unexpected(_))));
  }
  let s=MockServer::start().await;Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_string("not JSON")).mount(&s).await;
  assert!(matches!(CloudflareClient::new(s.uri(),"api-secret").tunnel_token("a","t").await,Err(CfError::Unexpected(_))));
  assert!(matches!(CloudflareClient::new("http://127.0.0.1:0","api-secret").tunnel_token("a","t").await,Err(CfError::Network(_))));
 }
 #[tokio::test] async fn api_log_redacts_api_and_tunnel_tokens_including_provider_errors() {
  let s=MockServer::start().await;let api="fabricated-api-secret-123";let tunnel="fabricated-tunnel-secret-456";
  Mock::given(path("/accounts/a/cfd_tunnel/t/token")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"success":true,"result":tunnel}))).mount(&s).await;
  Mock::given(path("/accounts/a/cfd_tunnel/t/configurations")).respond_with(ResponseTemplate::new(403).set_body_json(json!({"success":false,"errors":[{"code":9103,"message":format!("denied {api} {tunnel}\nforbidden")}],"result":{"secret":tunnel}}))).mount(&s).await;
  let logs=Arc::new(Mutex::new(Vec::<String>::new()));let capture=logs.clone();
  let c=CloudflareClient::new(s.uri(),api).with_logger(move|line|capture.lock().unwrap().push(line.to_owned()));
  assert_eq!(c.tunnel_token("a","t").await.unwrap(),tunnel);assert!(c.get_ingress("a","t").await.is_err());
  let logs=logs.lock().unwrap();assert_eq!(logs.len(),2);
  let all=logs.join("\n");assert!(!all.contains(api));assert!(!all.contains(tunnel));assert!(!all.contains("Authorization"));assert!(!all.contains("result"));
  assert!(all.contains("status=200"));assert!(all.contains("status=403"));assert!(all.contains("9103"));assert!(all.contains("denied"));assert!(all.contains("duration_ms="));assert_eq!(all.lines().count(),2);
 }
 #[tokio::test] async fn tunnel_delete_cleans_active_connections_then_retries_and_missing_is_success() {
  use std::sync::atomic::{AtomicUsize,Ordering};
  let s=MockServer::start().await;let attempts=Arc::new(AtomicUsize::new(0));let count=attempts.clone();
  Mock::given(method("DELETE")).and(path("/accounts/a/cfd_tunnel/t")).respond_with(move|_:&wiremock::Request|if count.fetch_add(1,Ordering::SeqCst)==0 {ResponseTemplate::new(400).set_body_json(json!({"success":false,"errors":[{"code":1022,"message":"active connections"}]}))}else{ResponseTemplate::new(200).set_body_json(json!({"success":true,"result":{}}))}).expect(2).mount(&s).await;
  Mock::given(method("DELETE")).and(path("/accounts/a/cfd_tunnel/t/connections")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"success":true,"result":{}}))).expect(1).mount(&s).await;
  Mock::given(method("DELETE")).and(path("/accounts/a/cfd_tunnel/missing")).respond_with(ResponseTemplate::new(404)).mount(&s).await;
  let c=CloudflareClient::new(s.uri(),"api-secret");c.delete_tunnel("a","t").await.unwrap();c.delete_tunnel("a","missing").await.unwrap();assert_eq!(attempts.load(Ordering::SeqCst),2);
 }
 #[tokio::test] async fn stable_tunnel_search_paginates_and_ignores_deleted() {
  let s=MockServer::start().await;
  for (page,result) in [(1,json!([{"id":"deleted","name":"hostify-s","deleted_at":"yesterday"}])),(2,json!([{"id":"t","name":"hostify-s","deleted_at":null}]))] {
   Mock::given(path("/accounts/a/cfd_tunnel")).and(query_param("page",page.to_string())).respond_with(ResponseTemplate::new(200).set_body_json(json!({"success":true,"result":result,"result_info":{"total_pages":2}}))).expect(1).mount(&s).await;
  }
  assert_eq!(CloudflareClient::new(s.uri(),"api-secret").find_tunnel("a","hostify-s").await.unwrap().as_deref(),Some("t"));
 }
 #[tokio::test] async fn finds_zone_at_apex_and_dns_readback_must_be_proxied() {
  let s=MockServer::start().await;
  Mock::given(path("/zones")).and(query_param("name","example.com")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"success":true,"result":[{"id":"z","name":"example.com"}]}))).mount(&s).await;
  Mock::given(path("/zones/z/dns_records/r")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"success":true,"result":{"content":"t.cfargotunnel.com","proxied":false}}))).mount(&s).await;
  let c=CloudflareClient::new(s.uri(),"api-secret");assert_eq!(c.find_zone("example.com").await.unwrap().id,"z");
  assert!(matches!(c.verify_dns("z","r","t").await,Err(CfError::Unexpected(_))));
 }
}
