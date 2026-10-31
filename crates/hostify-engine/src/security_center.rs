//! Local evidence-only security posture. No token reading, DNS writes, or repairs.
//! "pass" means a *local deterministic* invariant only; external verification
//! remains "unverified" and never becomes a green success from saved metadata.
use crate::state::{Engine, Site};
use anyhow::Result;
use chrono::Utc;
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub site_id: Option<String>,
    pub id: &'static str,
    pub severity: &'static str,
    pub status: &'static str,
    pub title: &'static str,
    pub detail: &'static str,
    pub recommended_action: &'static str,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Posture {
    pub checked_at: String,
    pub inspected_sites: usize,
    pub blocking_findings: usize,
    pub warning_findings: usize,
    pub unverified_checks: usize,
    pub findings: Vec<Finding>,
}
fn finding(site: Option<&str>, id: &'static str, severity: &'static str,
    status: &'static str, title: &'static str, detail: &'static str,
    recommended_action: &'static str) -> Finding {
    Finding {site_id:site.map(str::to_owned),id,severity,status,title,detail,recommended_action}
}
#[derive(Clone, Copy, Debug)]
struct Evidence {
    caddy_guard_configured: bool,
    service: bool,
    trusted_active: bool,
    has_local_snapshot: bool,
}
fn evaluate_site(site: &Site, e: Evidence) -> Vec<Finding> {
    let mut out=Vec::new();
    let dynamic = matches!(site.kind.as_str(),"node"|"php");
    let active = site.status=="running";
    if dynamic && active && !e.trusted_active {
        out.push(finding(Some(&site.id),"TRUSTED_RELEASE","critical","block",
            "Running application has no verified trusted active release",
            "The process status is running, but the active release is not both built and explicitly trusted.",
            "Stop this workload and complete explicit trust, build and release verification."));
    }
    if dynamic && e.service {
        out.push(finding(Some(&site.id),"PRIVILEGED_WORKLOAD","critical","block",
            "Privileged Windows service mode is unsafe for customer applications",
            "Shared Windows service identities must not execute arbitrary customer Node.js or PHP processes.",
            "Disable privileged hosting until a restricted per-site Windows process identity is implemented."));
    }
    if active && !e.caddy_guard_configured {
        out.push(finding(Some(&site.id),"HOST_GUARD_MISSING","critical","block",
            "Caddy host restriction configuration was not found",
            "The local Caddy configuration could not be verified to reject foreign Host headers.",
            "Regenerate and validate this site's Caddy configuration. Keep host isolation enabled."));
    }
    if dynamic {
        out.push(finding(Some(&site.id),"WINDOWS_TENANT_SANDBOX","high","warn",
            "Customer process isolation needs independent Windows verification",
            "A user-owned desktop runtime does not guarantee isolation between untrusted customers.",
            "Introduce restricted OS tokens, filesystem ACLs, quotas and cross-site penetration tests before resale."));
    }
    if site.public_url.is_some() {
        if !active {
            out.push(finding(Some(&site.id),"PUBLISHED_ORIGIN_STOPPED","high","block",
                "Published website is not currently confirmed running",
                "Historical HTTPS verification is not proof that the origin is healthy now.",
                "Restore local application health and rerun public HTTPS verification."));
        }
        out.push(finding(Some(&site.id),"EXTERNAL_HTTPS","medium","unverified",
            "Public HTTPS and external routing require live verification",
            "A saved domain record alone cannot verify DNS, TLS, Cloudflare Tunnel or public origin readiness.",
            "Run the live deployment verifier against an isolated test hostname."));
    }
    if !e.has_local_snapshot {
        out.push(finding(Some(&site.id),"NO_LOCAL_SNAPSHOT","medium","warn",
            "No local file snapshot is recorded",
            "No Time Machine file snapshot was found for this website.",
            "Create and verify a file snapshot. Back up databases and credentials through separate mechanisms."));
    }
    out
}
pub fn gather(engine:&Engine)->Result<Value>{
    let sites=engine.store.sites()?;
    let mut findings=Vec::new();
    for site in &sites {
        let caddy=engine.store.root.join("run").join(format!("{}.Caddyfile",site.id));
        let guard=std::fs::read_to_string(caddy)
            .ok().is_some_and(|text| text.contains("@foreign not host ") &&
                text.contains("respond @foreign \"Host not allowed\" 421") &&
                text.contains("bind 127.0.0.1"));
        let trusted_active=if matches!(site.kind.as_str(),"node"|"php"){
            let config=engine.store.app_config(&site.id)?;
            config.active_release.as_ref().is_some_and(|release|
                config.trusted_release.as_ref()==Some(release) &&
                config.built_release.as_ref()==Some(release))
        }else{true};
        let backup_base=engine.store.root.join("backups/site-snapshots");
        let has_local_snapshot=hostify_core::snapshots::list(&backup_base,&site.id)
            .is_ok_and(|records| !records.is_empty());
        findings.extend(evaluate_site(site,Evidence{
            caddy_guard_configured:guard,service:engine.service,trusted_active,has_local_snapshot
        }));
    }
    findings.push(finding(None,"CREDENTIAL_AUDIT","medium","unverified",
        "Credential rotation and off-device backup have not been verified",
        "This local scan does not inspect secret values, remote backup copies, or centralized account access.",
        "Verify secret storage, rotations and encrypted off-device backups before commercial deployment."));
    let posture=Posture {
        checked_at:Utc::now().to_rfc3339(),
        inspected_sites:sites.len(),
        blocking_findings:findings.iter().filter(|f|f.status=="block").count(),
        warning_findings:findings.iter().filter(|f|f.status=="warn").count(),
        unverified_checks:findings.iter().filter(|f|f.status=="unverified").count(),
        findings,
    };
    Ok(json!(posture))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn site(kind:&str,status:&str,public:bool)->Site{
        Site {
            id:"00000000-0000-4000-8000-000000000001".into(),name:"example".into(),
            kind:kind.into(),status:status.into(),public_url:public.then(||"https://example.test".into()),
            source_path:"C:/site".into(),local_port:18080,local_url:"http://127.0.0.1:18080".into(),
            runtime_version:None,publish_state:None,publish_deployment_id:None,publish_hostname:None,
            status_detail:None,domain_id:None,auto_start:false,file_count:0,size_bytes:0,
            created_at:"".into(),updated_at:"".into()
        }
    }
    #[test]
    fn published_stopped_is_never_healthy_and_static_unverified(){
        let findings=evaluate_site(&site("static","stopped",true),Evidence{
            caddy_guard_configured:true,service:false,trusted_active:true,has_local_snapshot:true
        });
        assert!(findings.iter().any(|f|f.id=="PUBLISHED_ORIGIN_STOPPED"&&f.status=="block"));
        assert!(findings.iter().any(|f|f.id=="EXTERNAL_HTTPS"&&f.status=="unverified"));
    }
    #[test]
    fn cannot_silently_accept_untrusted_privileged_dynamic_site(){
        let findings=evaluate_site(&site("node","running",false),Evidence{
            caddy_guard_configured:false,service:true,trusted_active:false,has_local_snapshot:false
        });
        for id in ["TRUSTED_RELEASE","PRIVILEGED_WORKLOAD","HOST_GUARD_MISSING"]{
            assert!(findings.iter().any(|f|f.id==id && f.status=="block"),"{id}");
        }
        let json=serde_json::to_string(&findings).unwrap();
        assert!(!json.contains("token"));
        assert!(!json.contains("password"));
    }
    #[test]
    fn local_passes_do_not_claim_public_readiness(){
        let findings=evaluate_site(&site("static","running",true),Evidence{
            caddy_guard_configured:true,service:false,trusted_active:true,has_local_snapshot:true
        });
        assert_eq!(findings.len(),1);
        assert_eq!(findings[0].status,"unverified");
    }
}
