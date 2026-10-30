use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
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
    #[error("Cloudflare error {code}: {message}")] Api { code: i64, message: String },
}
impl CfError { pub fn friendly(&self) -> String { self.to_string() } }
#[derive(Clone)]
pub struct CloudflareClient { base: String, token: String, client: reqwest::Client }
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
        v["result"]["id"].as_str().map(str::to_owned).ok_or_else(|| CfError::Network("Missing tunnel ID".into()))
    }
    pub async fn tunnel_token(&self, account: &str, id: &str) -> Result<String, CfError> {
        let v = self.request(reqwest::Method::GET, &format!("/accounts/{account}/cfd_tunnel/{id}/token"), None).await?;
        v["result"].as_str().map(str::to_owned).ok_or_else(|| CfError::Network("Missing tunnel token".into()))
    }
    pub async fn get_ingress(&self, account: &str, id: &str) -> Result<Value, CfError> {
        let v = self.request(reqwest::Method::GET, &format!("/accounts/{account}/cfd_tunnel/{id}/configurations"), None).await?;
        let config = v["result"]["config"].clone();
        if !config["ingress"].is_array() { return Err(CfError::Network("Missing tunnel ingress".into())); }
        Ok(config)
    }
    pub async fn put_config(&self, account: &str, id: &str, config: Value) -> Result<(), CfError> {
        self.request(reqwest::Method::PUT, &format!("/accounts/{account}/cfd_tunnel/{id}/configurations"), Some(json!({"config":config}))).await?; Ok(())
    }
    pub async fn dns_preflight(&self, zone: &str, hostname: &str) -> Result<(), CfError> {
        for v in self.records(zone, hostname).await? {
            if v["type"] != "CNAME" || v["comment"] != "managed-by=hostify" { return Err(CfError::Conflict); }
        }
        Ok(())
    }
    pub async fn find_deployment_dns(&self, zone: &str, hostname: &str, owner: &str) -> Result<Option<String>, CfError> {
        Ok(self.records(zone, hostname).await?.iter().find(|v| v["comment"] == "managed-by=hostify" && v["tags"].as_array().is_some_and(|tags| tags.contains(&json!(format!("hostify-deployment:{owner}"))))).and_then(|v| v["id"].as_str()).map(str::to_owned))
    }
    pub async fn create_deployment_dns(&self, zone: &str, hostname: &str, tunnel: &str, owner: &str) -> Result<String, CfError> {
        if self.plan_dns(zone, hostname, tunnel).await?.action != "create" { return Err(CfError::Conflict); }
        let v = self.request(reqwest::Method::POST, &format!("/zones/{zone}/dns_records"), Some(json!({"type":"CNAME","name":hostname,"content":format!("{tunnel}.cfargotunnel.com"),"proxied":true,"comment":"managed-by=hostify","tags":[format!("hostify-deployment:{owner}")]}))).await?;
        v["result"]["id"].as_str().map(str::to_owned).ok_or_else(|| CfError::Network("Missing DNS record ID".into()))
    }
    pub fn new(base_url: impl AsRef<str>, token: impl Into<String>) -> Self { Self { base: base_url.as_ref().trim_end_matches('/').to_owned(), token: token.into(), client: reqwest::Client::builder().timeout(Duration::from_secs(20)).build().unwrap_or_default() } }
    pub fn production(token: impl Into<String>) -> Self { Self::new("https://api.cloudflare.com/client/v4", token) }
    async fn request(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value, CfError> {
        let mut req = self.client.request(method, format!("{}{path}", self.base)).bearer_auth(&self.token);
        if let Some(v) = body { req = req.json(&v); }
        let response = req.send().await.map_err(|e| CfError::Network(e.to_string()))?;
        let status = response.status();
        if status.as_u16() == 404 { return Err(CfError::NotFound); }
        if status.as_u16() == 401 { return Err(CfError::InvalidToken); }
        if status.as_u16() == 403 { return Err(CfError::MissingPermission("access to this resource".into())); }
        if status.as_u16() == 429 { return Err(CfError::RateLimited); }
        if status.as_u16() == 409 { return Err(CfError::Conflict); }
        let v: Value = response.json().await.map_err(|e| CfError::Network(e.to_string()))?;
        if !status.is_success() || v.get("success").and_then(Value::as_bool) != Some(true) {
            let e = v.get("errors").and_then(Value::as_array).and_then(|a| a.first());
            let code = e.and_then(|x| x.get("code")).and_then(Value::as_i64).unwrap_or(0);
            let message = e.and_then(|x| x.get("message")).and_then(Value::as_str).unwrap_or("Unknown API error").to_owned();
            return Err(match code { 9109 | 10000 => CfError::InvalidToken, 9103 => CfError::MissingPermission(message), _ => CfError::Api { code, message } });
        }
        Ok(v)
    }
    async fn pages(&self, path: &str) -> Result<Vec<Value>, CfError> {
        let mut page = 1; let mut all = Vec::new();
        loop { let sep = if path.contains('?') { '&' } else { '?' }; let v = self.request(reqwest::Method::GET, &format!("{path}{sep}per_page=100&page={page}"), None).await?;
            let items = v.get("result").and_then(Value::as_array).ok_or_else(|| CfError::Network("Invalid list response".into()))?; all.extend(items.iter().cloned());
            let total = v.pointer("/result_info/total_pages").and_then(Value::as_u64).unwrap_or(1); if page >= total { break; } page += 1;
        } Ok(all)
    }
    pub async fn verify_token(&self) -> Result<(), CfError> { let v=self.request(reqwest::Method::GET, "/user/tokens/verify", None).await?; if v.pointer("/result/status").and_then(Value::as_str) != Some("active") { return Err(CfError::InvalidToken); } Ok(()) }
    pub async fn list_accounts(&self) -> Result<Vec<Account>, CfError> { self.pages("/accounts").await?.into_iter().map(|v| serde_json::from_value(v).map_err(|e| CfError::Network(e.to_string()))).collect() }
    pub async fn find_zone(&self, hostname: &str) -> Result<Zone, CfError> {
        let labels: Vec<_> = hostname.trim_end_matches('.').split('.').collect();
        for i in 0..labels.len().saturating_sub(1) { let name=labels[i..].join("."); let url = reqwest::Url::parse_with_params("http://localhost/", &[("name", &name)]).map_err(|e| CfError::Network(e.to_string()))?; let path = format!("/zones?{}", url.query().unwrap_or(""));
            for v in self.pages(&path).await? { let z: Zone=serde_json::from_value(v).map_err(|e| CfError::Network(e.to_string()))?; if z.name.eq_ignore_ascii_case(&name) { return Ok(z); } }
        } Err(CfError::ZoneNotFound)
    }
    pub async fn check_dns_permission(&self, zone: &str) -> Result<(), CfError> { self.pages(&format!("/zones/{zone}/dns_records")).await.map(|_| ()) }
    pub async fn get_or_create_tunnel(&self, account: &str, name: &str) -> Result<Tunnel, CfError> {
        let path=format!("/accounts/{account}/cfd_tunnel"); let tunnels=self.pages(&path).await?;
        let existing=tunnels.iter().find(|v| v.get("name").and_then(Value::as_str)==Some(name) && v.get("deleted_at").is_none_or(Value::is_null));
        let (v, created)=match existing { Some(v)=>(v.clone(),false), None=>(self.request(reqwest::Method::POST,&path,Some(json!({"name":name,"config_src":"cloudflare"}))).await?["result"].clone(),true) };
        let id=v.get("id").and_then(Value::as_str).ok_or_else(|| CfError::Network("Missing tunnel ID".into()))?.to_owned();
        let token=self.request(reqwest::Method::GET,&format!("{path}/{id}/token"),None).await?["result"].as_str().ok_or_else(|| CfError::Network("Missing tunnel token".into()))?.to_owned(); Ok(Tunnel{id,token,created})
    }
    pub async fn delete_tunnel(&self, account: &str, id: &str) -> Result<(), CfError> { self.request(reqwest::Method::DELETE,&format!("/accounts/{account}/cfd_tunnel/{id}"),None).await.map(|_| ()) }
    pub async fn put_tunnel_ingress(&self, account: &str, id: &str, entries: &[(String,String)]) -> Result<(), CfError> { let mut ingress: Vec<Value>=entries.iter().map(|(hostname,service)| json!({"hostname":hostname,"service":service})).collect(); ingress.push(json!({"service":"http_status:404"})); self.request(reqwest::Method::PUT,&format!("/accounts/{account}/cfd_tunnel/{id}/configurations"),Some(json!({"config":{"ingress":ingress}}))).await.map(|_| ()) }
    async fn records(&self, zone: &str, hostname: &str) -> Result<Vec<Value>, CfError> { let url=reqwest::Url::parse_with_params("http://localhost/", &[("name",hostname)]).map_err(|e| CfError::Network(e.to_string()))?; self.pages(&format!("/zones/{zone}/dns_records?{}",url.query().unwrap_or(""))).await }
    pub async fn plan_dns(&self, zone: &str, hostname: &str, tunnel: &str) -> Result<DnsPlan, CfError> {
        let target=format!("{tunnel}.cfargotunnel.com"); let mut own=None;
        for v in self.records(zone,hostname).await? { let kind=v["type"].as_str().unwrap_or(""); if !["A","AAAA","CNAME","TXT"].contains(&kind) { continue; } let content=v["content"].as_str().unwrap_or(""); let existing=Some(ExistingRecord{kind:kind.into(),content:content.into()}); if kind=="CNAME" && content.eq_ignore_ascii_case(&target) && v["comment"].as_str()==Some("managed-by=hostify") { own=existing; } else { return Ok(DnsPlan{hostname:hostname.into(),action:"conflict".into(),existing,message:"An existing DNS record conflicts with this hostname".into()}); } }
        Ok(DnsPlan{hostname:hostname.into(),action:if own.is_some(){"update_own"}else{"create"}.into(),existing:own,message:"DNS record can be configured".into()})
    }
    pub async fn create_dns(&self, zone: &str, hostname: &str, tunnel: &str) -> Result<String, CfError> { if self.plan_dns(zone,hostname,tunnel).await?.action != "create" { return Err(CfError::Conflict); } let v=self.request(reqwest::Method::POST,&format!("/zones/{zone}/dns_records"),Some(json!({"type":"CNAME","name":hostname,"content":format!("{tunnel}.cfargotunnel.com"),"proxied":true,"comment":"managed-by=hostify"}))).await?; v["result"]["id"].as_str().map(str::to_owned).ok_or_else(|| CfError::Network("Missing DNS record ID".into())) }
    pub async fn delete_dns_if_ours(&self, zone: &str, id: &str) -> Result<bool, CfError> { let path=format!("/zones/{zone}/dns_records/{id}"); let v=match self.request(reqwest::Method::GET,&path,None).await { Ok(v)=>v, Err(CfError::NotFound)=>return Ok(false), Err(e)=>return Err(e) }; if v["result"]["comment"].as_str()!=Some("managed-by=hostify") { return Ok(false); } self.request(reqwest::Method::DELETE,&path,None).await?; Ok(true) }
}
#[cfg(test)] mod tests {
    use super::*;
    use wiremock::{MockServer, Mock, ResponseTemplate, matchers::{method,path,query_param}};
    #[tokio::test] async fn token_and_permission_errors() {
        let s=MockServer::start().await;
        Mock::given(method("GET")).and(path("/user/tokens/verify")).respond_with(ResponseTemplate::new(401)).mount(&s).await;
        Mock::given(method("GET")).and(path("/zones/z/dns_records")).respond_with(ResponseTemplate::new(403)).mount(&s).await;
        let c=CloudflareClient::new(s.uri(),"secret"); assert!(matches!(c.verify_token().await,Err(CfError::InvalidToken))); assert!(matches!(c.check_dns_permission("z").await,Err(CfError::MissingPermission(_))));
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
 assert_eq!(CloudflareClient::new(s.uri(),"token").create_dns("z","www.example.com","t").await.expect("create"),"r"); }
}
