//! Public verification collects evidence without mutating Cloudflare resources.
use async_trait::async_trait;
use chrono::{
    DateTime,
    Utc
};
use serde::{
    Serialize,
    Deserialize
};
use std::{
    net::{
        IpAddr,
        SocketAddr
    },
    sync::Arc,
    time::Duration
};
use crate::{
    cloudflare::{
        CloudflareClient,
        Zone
    },
    deploy::SiteRuntime
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentState {
    #[serde(alias = "running")] Validating,
    StartingLocal,
    ProvisioningTunnel,
    ConfiguringDns,
    StartingConnector,
    WaitingDns,
    WaitingTls,
    VerifyingPublic,
    #[serde(alias = "live")] Published,
    Failed,
    RecoveryRequired,
    RolledBack
}

impl DeploymentState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Validating => "validating",
            Self::StartingLocal => "starting_local",
            Self::ProvisioningTunnel => "provisioning_tunnel",
            Self::ConfiguringDns => "configuring_dns",
            Self::StartingConnector => "starting_connector",
            Self::WaitingDns => "waiting_dns",
            Self::WaitingTls => "waiting_tls",
            Self::VerifyingPublic => "verifying_public",
            Self::Published => "published",
            Self::Failed => "failed",
            Self::RecoveryRequired => "recovery_required",
            Self::RolledBack => "rolled_back",
        }
    }
}

impl From<&str> for DeploymentState {
    fn from(value: &str) -> Self {
        serde_json::from_value(serde_json::Value::String(value.into())).expect("internal deployment state must match IPC")
    }
}

impl std::ops::Deref for DeploymentState {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl PartialEq<&str> for DeploymentState {
    fn eq(&self, other: &&str) -> bool {
        self.as_str()==*other
    }
}

impl std::fmt::Display for DeploymentState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl rusqlite::ToSql for DeploymentState {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(self.as_str().into())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifyClass {
    ZoneNotActive,
    DnsRecordMissing,
    DnsNotVisible,
    TlsPending,
    ConnectorDown,
    OriginUnreachable,
    RoutingMismatch,
    NetworkError,
    CloudflareApi
}

impl VerifyClass {
    pub fn state(self) -> &'static str {
        match self {
            Self::ZoneNotActive|Self::DnsRecordMissing|Self::DnsNotVisible => "waiting_dns",
            Self::TlsPending => "waiting_tls",
            _ => "verifying_public",
        }
    }
    pub fn message(self) -> &'static str {
        match self {
            Self::ZoneNotActive => "Cloudflare has not activated the zone. Set the domain's nameservers to the nameservers shown here.",
            Self::DnsRecordMissing => "The DNS record was removed or changed. Review it in Cloudflare; Hostify will not overwrite it.",
            Self::DnsNotVisible => "No resolver returned a public edge address yet. Hostify will check again automatically.",
            Self::TlsPending => "Cloudflare's HTTPS certificate or handshake is not ready. Hostify will check again automatically.",
            Self::ConnectorDown => "The Cloudflare connector is not connected. Check tunnel logs and this PC's network connection.",
            Self::OriginUnreachable => "Cloudflare cannot reach the local website. Check the local server and tunnel logs.",
            Self::RoutingMismatch => "The public route did not return this deployment's marker. Review hostname and tunnel routing in Cloudflare.",
            Self::NetworkError => "The public HTTPS connection failed or timed out. Check network access; Hostify will retry.",
            Self::CloudflareApi => "Cloudflare's API could not confirm the configuration. Check token permissions and API access; Hostify will retry.",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyCheck {
    pub key: String,
    pub label: String,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")] pub detail: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Verification {
    pub attempt: u32,
    #[serde(skip_serializing_if = "Option::is_none")] pub next_retry_at: Option<String>,
    pub deadline_at: String,
    #[serde(skip_serializing_if = "Option::is_none")] pub error_class: Option<VerifyClass>,
    #[serde(skip_serializing_if = "Option::is_none")] pub message: Option<String>,
    pub checks: Vec<VerifyCheck>,
    #[serde(skip_serializing_if = "Option::is_none")] pub last_checked_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub visible_from_this_pc: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")] pub nameservers: Option<Vec<String>>,
}

impl Verification {
    pub fn new(deadline: DateTime<Utc>) -> Self {
        Self {
            attempt: 0,
            next_retry_at: None,
            deadline_at: deadline.to_rfc3339(),
            error_class: None,
            message: None,
            checks: Vec::new(),
            last_checked_at: None,
            visible_from_this_pc: None,
            nameservers: None
        }
    }
    fn check(&mut self, index: usize, passed: bool, detail: impl Into<String>) {
        self.checks[index].state = if passed {
            "passed"
        } else {
            "failed"
        }
        .into();
        self.checks[index].detail = Some(detail.into());
    }
    fn fail(&mut self, index: usize, class: VerifyClass, detail: impl Into<String>) {
        self.check(index, false, detail);
        self.error_class = Some(class);
        self.message = Some(class.message().into());
    }
}
pub const BUDGET_SECONDS: i64 = 1800;
pub fn budget_expired(now: DateTime<Utc>, deadline: DateTime<Utc>) -> bool {
    now>=deadline
}
/// Cap the exponential base at 60 seconds, then apply +/-20% jitter.
pub fn backoff(attempt: u32, random: u8) -> Duration {
    let base = 5u64.saturating_mul(1u64<<attempt.saturating_sub(1).min(4)).min(60);
    Duration::from_millis(base*(800+u64::from(random)*400/255))
}

#[derive(Clone, Debug)]
pub struct HttpEvidence {
    pub marker_status: u16,
    pub marker_body: String,
    pub root_status: u16
}

impl HttpEvidence {
    fn classify(&self, nonce: &str) -> Option<VerifyClass> {
        let connector_error = self.marker_status == 530 || self.root_status == 530
            || (self.marker_body.contains("1033")
                && (self.marker_status != 200
                    || self.marker_body.to_ascii_lowercase().contains("cloudflare")));
        if connector_error {
            Some(VerifyClass::ConnectorDown)
        } else if [502, 503, 504].contains(&self.marker_status)
            || [502, 503, 504].contains(&self.root_status) {
            Some(VerifyClass::OriginUnreachable)
        } else if self.marker_status != 200 || self.marker_body != nonce
            || !(200..300).contains(&self.root_status) {
            Some(VerifyClass::RoutingMismatch)
        } else {
            None
        }
    }
}

#[async_trait]
pub trait PublicProbe: Send+Sync {
    async fn zone(&self, zone: &str) -> Result<Zone, String>;
    async fn record(&self, zone: &str, hostname: &str) -> Result<Option<serde_json::Value>, String>;
    async fn connector(&self, tunnel: &str) -> Result<(bool, u64), String>;
    async fn authoritative(&self, hostname: &str, nameserver: &str) -> Result<Vec<IpAddr>, String>;
    async fn recursive(&self, hostname: &str, endpoint: &str) -> Result<Vec<IpAddr>, String>;
    async fn system(&self, hostname: &str) -> Result<Vec<IpAddr>, String>;
    async fn https(&self, hostname: &str, address: IpAddr, nonce: &str) -> Result<HttpEvidence, VerifyClass>;
}
pub struct NetworkProbe {
    pub cf: CloudflareClient,
    pub runtime: Arc<dyn SiteRuntime>
}
async fn bounded<T>(future: impl std::future::Future<Output = Result<T, String>>) -> Result<T, String> {
    tokio::time::timeout(Duration::from_secs(10), future).await.map_err(|_|"Observation timed out".to_string())?
}

pub async fn attempt(
    probe: &dyn PublicProbe,
    zone_id: &str,
    hostname: &str,
    record_id: Option<&str>,
    tunnel: &str,
    nonce: &str,
    previous: &Verification,
) -> Verification {
    let mut v = previous.clone();
    v.attempt = v.attempt.saturating_add(1);
    v.last_checked_at = Some(Utc::now().to_rfc3339());
    v.next_retry_at = None;
    v.error_class = None;
    v.visible_from_this_pc = None;
    v.nameservers = None;
    v.message = None;
    v.checks = [("zone","Cloudflare zone"), ("dns_record","DNS record"), ("connector","Tunnel connector"), ("dns_public","Public DNS"), ("tls","HTTPS certificate"), ("http","Public HTTP"), ("content","Deployment marker")].iter().map(|(key, label)|VerifyCheck {
        key: (*key).into(),
        label: (*label).into(),
        state: "pending".into(),
        detail: None
    }
    ).collect();
    let zone = match bounded(probe.zone(zone_id)).await {
        Ok(z) => z,
        Err(e) => {
            v.fail(0, VerifyClass::CloudflareApi, e);
            return v;
        }
    };
    v.nameservers = Some(zone.name_servers.clone());
    if zone.status!="active" {
        v.fail(0, VerifyClass::ZoneNotActive, zone.status);
        return v;
    }
    v.check(0, true,"Zone is active");
    match bounded(probe.record(zone_id, hostname)).await {
        Ok(Some(r)) if record_id.is_none_or(|id|r["id"]==id) && r["type"]=="CNAME" && r["proxied"]==true && r["content"].as_str().is_some_and(|s|s.trim_end_matches('.').eq_ignore_ascii_case(&format!("{tunnel}.cfargotunnel.com"))) => v.check(1, true,"Journaled proxied tunnel record confirmed"),
        Ok(_) => {
            v.fail(1, VerifyClass::DnsRecordMissing,"Record missing or changed");
            return v;
        }
        Err(e) => {
            v.fail(1, VerifyClass::CloudflareApi, e);
            return v;
        }
    }
    match bounded(probe.connector(tunnel)).await {
        Ok((true, n)) if n>0 => v.check(2, true, format!("{n} connections")),
        other => {
            v.fail(2, VerifyClass::ConnectorDown, format!("{other:?}"));
            return v;
        }
    }
    let mut addresses = Vec::new();
    let mut observations = Vec::new();
    for ns in &zone.name_servers {
        let result = bounded(probe.authoritative(hostname, ns)).await;
        observations.push(format!("{ns}: {result:?}"));
        addresses.extend(result.unwrap_or_default());
    }
    for endpoint in ["https://cloudflare-dns.com/dns-query","https://dns.google/resolve"] {
        let result = bounded(probe.recursive(hostname, endpoint)).await;
        observations.push(format!("{endpoint}: {result:?}"));
        addresses.extend(result.unwrap_or_default());
    }
    let system = bounded(probe.system(hostname)).await;
    v.visible_from_this_pc = Some(system.as_ref().is_ok_and(|a|!a.is_empty()));
    observations.push(format!("This PC: {system:?}"));
    addresses.extend(system.unwrap_or_default());
    addresses.sort_by_key(|ip|(ip.is_ipv6(), *ip));
    addresses.dedup();
    if addresses.is_empty() {
        v.fail(3, VerifyClass::DnsNotVisible, observations.join("; "));
        return v;
    }
    v.check(3, true, observations.join("; "));
    let mut failure = (4, VerifyClass::NetworkError,"No edge responded".to_string());
    for ip in addresses.into_iter().take(4) {
        match tokio::time::timeout(Duration::from_secs(10), probe.https(hostname, ip, nonce)).await {
            Ok(Ok(e)) => {
                let class = e.classify(nonce);
                if let Some(class) = class {
                    let failed_check = if e.marker_status!=200 { 5 } else { 6 };
                    failure = (failed_check, class, format!("{ip}: marker HTTP {}, root HTTP {}", e.marker_status, e.root_status));
                    continue;
                }
                v.check(4, true, format!("Certificate and handshake validated at {ip}"));
                v.check(5, true,"Marker HTTP 200; website HTTP 2xx");
                v.check(6, true,"Exact deployment nonce confirmed");
                v.message = Some(if v.visible_from_this_pc==Some(false) {
                    "Published. This PC is showing an old cached DNS answer that clears by itself."
                } else {
                    "Published. Public website and deployment marker verified."
                }
                .into());
                return v;
            }
            Ok(Err(class)) => failure = (4, class, format!("{ip}: {}", class.message())),
            Err(_) => failure = (4, VerifyClass::NetworkError, format!("{ip}: HTTPS timed out")),
        }
    }
    if failure.0>4 {
        v.check(4, true,"Certificate and handshake validated");
    }
    if failure.0>5 {
        v.check(5, true,"HTTP response received");
    }
    v.fail(failure.0, failure.1, failure.2);
    v
}

fn request_error(error: reqwest::Error) -> VerifyClass {
    use std::error::Error;
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(e) = source {
        text.push_str(&format!(" {e}"));
        source = e.source();
    }
    let text = text.to_lowercase();
    if ["certificate","handshake","tls","issuer"].iter().any(|s|text.contains(s)) {
        VerifyClass::TlsPending
    } else {
        VerifyClass::NetworkError
    }
}

#[async_trait]

impl PublicProbe for NetworkProbe {
    async fn zone(&self, id: &str) -> Result<Zone, String> {
        self.cf.zone(id).await.map_err(|e|e.to_string())
    }
    async fn record(&self, z: &str, h: &str) -> Result<Option<serde_json::Value>, String> {
        self.cf.dns_record(z, h).await.map_err(|e|e.to_string())
    }
    async fn connector(&self, t: &str) -> Result<(bool, u64), String> {
        self.runtime.connector_health(t).await
    }
    async fn system(&self, h: &str) -> Result<Vec<IpAddr>, String> {
        Ok(tokio::net::lookup_host((h, 443)).await.map_err(|e|e.to_string())?.map(|a|a.ip()).collect())
    }
    async fn recursive(&self, hostname: &str, endpoint: &str) -> Result<Vec<IpAddr>, String> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(4))
            .build()
            .map_err(|error| error.to_string())?;
        let mut addresses = Vec::new();
        let mut errors = Vec::new();
        for kind in ["A", "AAAA"] {
            let observation = async {
                client.get(endpoint)
                    .header("Accept", "application/dns-json")
                    .query(&[("name", hostname), ("type", kind)])
                    .send().await?
                    .error_for_status()?
                    .json::<serde_json::Value>().await
            }.await;
            match observation {
                Ok(response) => {
                    if let Some(answers) = response["Answer"].as_array() {
                        addresses.extend(answers.iter()
                            .filter(|record| record["type"] == 1 || record["type"] == 28)
                            .filter_map(|record| record["data"].as_str()?.parse::<IpAddr>().ok()));
                    }
                }
                Err(error) => errors.push(error.to_string()),
            }
        }
        if addresses.is_empty() && !errors.is_empty() {
            Err(errors.join("; "))
        } else {
            Ok(addresses)
        }
    }

    async fn authoritative(&self, hostname: &str, nameserver: &str) -> Result<Vec<IpAddr>, String> {
        let lookup = tokio::time::timeout(Duration::from_secs(2),
            tokio::net::lookup_host((nameserver, 53))).await
            .map_err(|_| "Nameserver lookup timed out".to_string())?
            .map_err(|error| error.to_string())?;
        let mut servers: Vec<_> = lookup.collect();
        servers.sort_by_key(|address| address.is_ipv6());
        let mut addresses = Vec::new();
        let mut errors = Vec::new();
        for server in servers.into_iter().take(2) {
            let bind = if server.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" };
            let socket = tokio::net::UdpSocket::bind(bind).await.map_err(|error| error.to_string())?;
            socket.connect(server).await.map_err(|error| error.to_string())?;
            for kind in [1, 28] {
                let random = uuid::Uuid::new_v4();
                let id = u16::from_be_bytes([random.as_bytes()[0], random.as_bytes()[1]]);
                let observation = async {
                    socket.send(&dns_query(hostname, kind, id)?).await.map_err(|error| error.to_string())?;
                    let mut packet = [0; 4096];
                    let length = tokio::time::timeout(Duration::from_secs(1), socket.recv(&mut packet)).await
                        .map_err(|_| "Authoritative DNS timed out".to_string())?
                        .map_err(|error| error.to_string())?;
                    if packet[..length].get(..2) != Some(&id.to_be_bytes()) {
                        return Err("DNS transaction ID did not match".into());
                    }
                    dns_answers(&packet[..length])
                }.await;
                match observation {
                    Ok(found) => addresses.extend(found),
                    Err(error) => errors.push(error),
                }
            }
        }
        if addresses.is_empty() && !errors.is_empty() {
            Err(errors.join("; "))
        } else {
            Ok(addresses)
        }
    }

    async fn https(&self, h: &str, ip: IpAddr, nonce: &str) -> Result<HttpEvidence, VerifyClass> {
        let hostname = h.to_owned();
        let client = reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(4))
        .resolve_to_addrs(h, &[SocketAddr::new(ip, 443)])
        .redirect(reqwest::redirect::Policy::custom(move |a| {
            if a.previous().len()>=5 || a.url().host_str()!=Some(&hostname) || a.url().scheme()!="https" || a.url().port_or_known_default()!=Some(443) {
                a.stop()
            } else {
                a.follow()
            }
        }
        )).build().map_err(request_error)?;
        let marker = client.get(format!("https://{h}/.well-known/hostify-verify/{nonce}")).send().await.map_err(request_error)?;
        let marker_status = marker.status().as_u16();
        let marker_body = marker.text().await.map_err(request_error)?;
        let root_status = if marker_status==200 && marker_body==nonce {
            client.get(format!("https://{h}/")).send().await.map_err(request_error)?.status().as_u16()
        } else {
            0
        };
        Ok(HttpEvidence {
            marker_status,
            marker_body,
            root_status
        })
    }
}
pub fn dns_query(host: &str, kind: u16, id: u16) -> Result<Vec<u8>, String> {
    let mut p = vec![];
    for n in [id, 0, 1, 0, 0, 0] {
        p.extend_from_slice(&n.to_be_bytes());
    }
    if host.len()>253 {
        return Err("DNS hostname too long".into());
    }
    for label in host.trim_end_matches('.').split('.') {
        if label.is_empty()||label.len()>63 {
            return Err("Invalid DNS label".into());
        }
        p.push(label.len() as u8);
        p.extend_from_slice(label.as_bytes());
    }
    p.push(0);
    p.extend_from_slice(&kind.to_be_bytes());
    p.extend_from_slice(&1u16.to_be_bytes());
    Ok(p)
}

fn word(p: &[u8], offset: usize) -> Result<u16, String> {
    let b = p.get(offset..offset+2).ok_or("Truncated DNS packet")?;
    Ok(u16::from_be_bytes([b[0], b[1]]))
}

fn name(p: &[u8], offset: &mut usize) -> Result<String, String> {
    let mut pos = *offset;
    let mut jumped = false;
    let mut labels = Vec::new();
    for _ in 0..128 {
        let n = *p.get(pos).ok_or("Truncated DNS name")?;
        if n&0xc0==0xc0 {
            let target = (word(p, pos)?&0x3fff) as usize;
            if !jumped {
                *offset = pos+2;
                jumped = true;
            }
            if target>=pos {
                return Err("Invalid DNS compression pointer".into());
            }
            pos = target;
            continue;
        }
        if n&0xc0!=0 {
            return Err("Invalid DNS label".into());
        }
        pos+=1;
        if n==0 {
            if !jumped {
                *offset = pos;
            }
            return Ok(labels.join("."));
        }
        let label = p.get(pos..pos+n as usize).ok_or("Truncated DNS label")?;
        labels.push(std::str::from_utf8(label).map_err(|_|"Invalid DNS name")?.to_ascii_lowercase());
        pos+=n as usize;
        if labels.iter().map(String::len).sum::<usize>() + labels.len().saturating_sub(1) > 253 {
            return Err("DNS name exceeds the maximum length".into());
        }
    }
    Err("DNS compression loop".into())
}
pub fn dns_answers(p: &[u8]) -> Result<Vec<IpAddr>, String> {
    if p.len()<12||word(p, 2)?&0x8000==0||word(p, 2)?&0x020f!=0 {
        return Err("Invalid, truncated or unsuccessful DNS response".into());
    }
    let mut offset = 12;
    let mut allowed = std::collections::HashSet::new();
    for _ in 0..word(p, 4)? {
        allowed.insert(name(p, &mut offset)?);
        offset+=4;
        if offset>p.len() {
            return Err("Truncated question".into());
        }
    }
    let mut answers = Vec::new();
    let mut aliases = Vec::new();
    for _ in 0..word(p, 6)? {
        let owner = name(p, &mut offset)?;
        let kind = word(p, offset)?;
        let class = word(p, offset+2)?;
        let len = word(p, offset+8)? as usize;
        offset+=10;
        let data = p.get(offset..offset+len).ok_or("Truncated DNS answer")?;
        if class==1 {
            match kind {
                1 if len==4 => answers.push((owner, IpAddr::from(<[u8;
                4]>::try_from(data).unwrap()))),
                28 if len==16 => answers.push((owner, IpAddr::from(<[u8;
                16]>::try_from(data).unwrap()))),
                5 => {
                    let mut end = offset;
                    let alias = name(p, &mut end)?;
                    if end!=offset+len {
                        return Err("Invalid CNAME length".into());
                    }
                    aliases.push((owner, alias));
                }
                1|28 => return Err("Invalid address length".into()),
                _ => (),
            }
        }
        offset+=len;
    }
    for _ in 0..usize::from(word(p, 8)?)+usize::from(word(p, 10)?) {
        name(p, &mut offset)?;
        let len = word(p, offset+8)? as usize;
        offset+=10;
        p.get(offset..offset+len).ok_or("Truncated DNS authority or additional record")?;
        offset+=len;
    }
    for _ in 0..aliases.len() {
        for (owner, alias) in &aliases {
            if allowed.contains(owner) {
                allowed.insert(alias.clone());
            }
        }
    }
    Ok(answers.into_iter().filter(|(owner, _)|allowed.contains(owner)).map(|(_, ip)|ip).collect())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn classes_states_messages_and_ipc_strings() {
        let classes = [VerifyClass::ZoneNotActive, VerifyClass::DnsRecordMissing, VerifyClass::DnsNotVisible, VerifyClass::TlsPending, VerifyClass::ConnectorDown, VerifyClass::OriginUnreachable, VerifyClass::RoutingMismatch, VerifyClass::NetworkError, VerifyClass::CloudflareApi];
        let strings = ["zone_not_active","dns_record_missing","dns_not_visible","tls_pending","connector_down","origin_unreachable","routing_mismatch","network_error","cloudflare_api"];
        for (i, c) in classes.into_iter().enumerate() {
            assert_eq!(serde_json::to_value(c).unwrap(), strings[i]);
            assert_eq!(c.state(), if i<3 {
                "waiting_dns"
            } else if i==3 {
                "waiting_tls"
            } else {
                "verifying_public"
            });
            assert!(!c.message().is_empty());
        }
        let states = [DeploymentState::Validating, DeploymentState::StartingLocal, DeploymentState::ProvisioningTunnel, DeploymentState::ConfiguringDns, DeploymentState::StartingConnector, DeploymentState::WaitingDns, DeploymentState::WaitingTls, DeploymentState::VerifyingPublic, DeploymentState::Published, DeploymentState::Failed, DeploymentState::RecoveryRequired, DeploymentState::RolledBack];
        let names = ["validating","starting_local","provisioning_tunnel","configuring_dns","starting_connector","waiting_dns","waiting_tls","verifying_public","published","failed","recovery_required","rolled_back"];
        for (state, name) in states.into_iter().zip(names) {
            assert_eq!(serde_json::to_value(state).unwrap(), name);
        }
    }
    #[test]
    fn exponential_backoff_bounds_and_budget() {
        for (attempt, base) in [(0, 5), (1, 5), (2, 10), (3, 20), (4, 40), (5, 60), (6, 60), (u32::MAX, 60)] {
            for random in 0..=255 {
                let delay = backoff(attempt, random).as_millis();
                assert!(delay>=base*800 && delay<=base*1200);
            }
            assert_eq!(backoff(attempt, 0).as_millis(), base*800);
            assert_eq!(backoff(attempt, 255).as_millis(), base*1200);
        }
        let now = Utc::now();
        assert!(budget_expired(now, now));
        assert!(budget_expired(now, now-chrono::Duration::seconds(1)));
        assert!(!budget_expired(now, now+chrono::Duration::seconds(BUDGET_SECONDS)));
    }
    fn answer(packet: &mut Vec<u8>, owner: &[u8], kind: u16, data: &[u8]) {
        packet.extend_from_slice(owner);
        for n in [kind, 1, 0, 30, data.len() as u16] {
            packet.extend_from_slice(&n.to_be_bytes());
        }
        packet.extend_from_slice(data);
    }
    #[test]
    fn dns_a_aaaa_cname_compression_and_malformed_packets() {
        let mut p = dns_query("example.com", 1, 42).unwrap();
        assert_eq!(&p[..12], &[0, 42, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
        p[2] = 0x84;
        p[7] = 3;
        let alias = [5, b'a', b'l', b'i', b'a', b's', 0xc0, 12];
        answer(&mut p, &[0xc0, 12], 5, &alias);
        answer(&mut p, &alias, 1, &[192, 0, 2, 1]);
        let ipv6 = "2001:db8::1".parse::<std::net::Ipv6Addr>().unwrap();
        answer(&mut p, &alias, 28, &ipv6.octets());
        assert_eq!(dns_answers(&p).unwrap(), vec!["192.0.2.1".parse::<IpAddr>().unwrap(), IpAddr::V6(ipv6)]);
        for n in 0..p.len() {
            assert!(dns_answers(&p[..n]).is_err(),"truncated at {n}");
        }
        let mut malformed = p.clone();
        malformed[2]|=2;
        assert!(dns_answers(&malformed).is_err());
        let mut malformed = p.clone();
        malformed[12] = 0xc0;
        malformed[13] = 12;
        assert!(dns_answers(&malformed).is_err());
        assert!(dns_query("", 1, 1).is_err());
        assert!(dns_query(&"x".repeat(64), 1, 1).is_err());
        for n in 0..512 {
            let junk = vec![255;
            n];
            assert!(std::panic::catch_unwind(||dns_answers(&junk)).is_ok());
        }
    }
    #[test]
    fn pinned_client_keeps_certificate_validation() {
        let source = include_str!("verify.rs");
        let implementation = source.split("#[cfg(test)]").next().unwrap();
        assert!(implementation.contains(".resolve_to_addrs("));
        assert!(!implementation.contains(&["danger_accept_","invalid_certs"].concat()));
        assert!(!implementation.contains(&["danger_accept_","invalid_hostnames"].concat()));
    }
    #[test]
    fn http_statuses_marker_identity_and_root_response_are_classified() {
        for (status,class) in [(530,VerifyClass::ConnectorDown),(502,VerifyClass::OriginUnreachable),(503,VerifyClass::OriginUnreachable),(504,VerifyClass::OriginUnreachable),(404,VerifyClass::RoutingMismatch)] {
            let e=HttpEvidence{marker_status:status,marker_body:"nonce".into(),root_status:200};
            assert_eq!(e.classify("nonce"),Some(class));
            let e=HttpEvidence{marker_status:200,marker_body:"nonce".into(),root_status:status};
            assert_eq!(e.classify("nonce"),Some(class));
        }
        let unrelated=HttpEvidence{marker_status:200,marker_body:"identical index.html".into(),root_status:200};
        assert_eq!(unrelated.classify("nonce"),Some(VerifyClass::RoutingMismatch));
        let cloudflare=HttpEvidence{marker_status:200,marker_body:"Cloudflare error 1033".into(),root_status:0};
        assert_eq!(cloudflare.classify("nonce"),Some(VerifyClass::ConnectorDown));
        let exact=HttpEvidence{marker_status:200,marker_body:"nonce".into(),root_status:204};
        assert_eq!(exact.classify("nonce"),None);
    }

}

