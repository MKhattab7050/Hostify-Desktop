//! Hostify Doctor: read-only, offline, deterministic diagnostic patterns.
//! No uploaded logs, arbitrary code execution, untrusted instructions or AI
//! inference. A future opt-in LLM layer can consume redacted structured findings.
use serde::Serialize;

#[derive(Clone,Debug,Serialize)]
#[serde(rename_all="camelCase")]
pub struct Issue {
    pub id: &'static str,
    pub severity: &'static str,
    pub category: &'static str,
    pub headline: &'static str,
    pub explanation: &'static str,
    pub action: &'static str,
    pub auto_fixed: bool,
}
#[derive(Clone,Debug,Serialize)]
#[serde(rename_all="camelCase")]
pub struct Report {
    pub issues: Vec<Issue>,
    pub lines_reviewed: usize,
    pub has_actionable_findings: bool,
    pub engine: &'static str,
}
fn issue(id:&'static str,severity:&'static str,category:&'static str,
    headline:&'static str,explanation:&'static str,action:&'static str)->Issue{
    Issue{id,severity,category,headline,explanation,action,auto_fixed:false}
}
pub fn analyze(logs:&[String],kind:&str,site_status:&str)->Report{
    // Only pattern matches, never echo source bytes. Even if a log contains
    // a credential, the generated report contains no raw log fragments.
    let text=logs.join("\n").to_ascii_lowercase();
    let mut issues=Vec::new();
    if text.contains("buffer-equal-constant-time") &&
        (text.contains("slowbuffer") || text.contains("reading 'prototype'") || text.contains("node.js v26")) {
        issues.push(issue("NODE26_LEGACY_DEP","high","RUNTIME",
            "Node.js 26 is incompatible with a dependency",
            "A legacy constant-time comparison dependency expects the removed SlowBuffer API.",
            "Select an approved Node.js 24 LTS runtime and rebuild only after explicit approval."));
    }
    if text.contains("eaddrinuse") || text.contains("address already in use") {
        issues.push(issue("PORT_CONFLICT","high","CONFIGURATION",
            "Application port is already occupied",
            "Another process owns a required local listener. Do not terminate unrelated software.",
            "Select an unused local port or review the current process; then restart safely."));
    }
    if text.contains("cannot find module") || text.contains("err_module_not_found") ||
       text.contains("module_not_found") {
        issues.push(issue("MISSING_MODULE","high","BUILD",
            "Application cannot load a required module",
            "A dependency may be absent, incompatible, or installed for a different runtime.",
            "Inspect package-lock and build logs; perform an approved clean build on a staged release."));
    }
    if text.contains("access denied for user") || text.contains("er_access_denied_error") {
        issues.push(issue("MYSQL_AUTH","high","DATABASE",
            "MySQL rejected application credentials",
            "The site's database user or connection settings do not match the configured account.",
            "Use the site's managed database settings to verify credentials. Never paste passwords into public logs."));
    }
    if text.contains("econnrefused") || text.contains("connection refused") {
        issues.push(issue("DEPENDENCY_OFFLINE","medium","CONNECTIVITY",
            "Application could not reach a required local service",
            "A local listener, database or dependency may be stopped or unreachable.",
            "Check the host, port, and health of the specific dependency before retrying."));
    }
    if text.contains("php fatal error") || text.contains("uncaught exception") {
        issues.push(issue("APPLICATION_EXCEPTION","high","APPLICATION",
            "Application terminated with an uncaught exception",
            "A PHP or application-level exception was reported; repeating deployment cannot fix it.",
            "Inspect the redacted application error log and correct the issue in a separate release."));
    }
    if text.contains("host not allowed") || text.contains("http 421") || text.contains("foreign host") {
        issues.push(issue("FOREIGN_HOST","high","SECURITY",
            "Caddy rejected an unexpected hostname",
            "The Host header did not match the configured allowed hostnames.",
            "Check the domain assignment and proxy Host header; never disable the security guard."));
    }
    if text.contains("dns") && (text.contains("propagation") || text.contains("not visible")) {
        issues.push(issue("DNS_PENDING","low","EXTERNAL_PROVIDER",
            "DNS propagation has not completed",
            "Public resolvers may not have received the most recent zone changes.",
            "Wait and run read-only DNS checks; do not recreate a record already owned by the deployment."));
    }
    if text.contains("disk full") || text.contains("no space left on device") ||
        text.contains("insufficient disk space") {
        issues.push(issue("LOW_DISK","high","STORAGE",
            "Not enough disk space for the deployment",
            "Downloads, builds and database backups require free storage.",
            "Free space or move data to a supported volume before retrying. Preserve existing backups."));
    }
    if site_status=="error" && issues.is_empty() {
        issues.push(issue("UNKNOWN_FAILURE","medium","APPLICATION",
            "Application requires investigation",
            "The site reports an error but no known signature was recognized.",
            "Inspect process state and the original redacted logs. No automated repair is safe yet."));
    }
    if kind=="static" && text.contains("index.html not found") {
        issues.push(issue("STATIC_ENTRY","medium","CONFIGURATION",
            "Static entry file missing",
            "The published root does not contain the expected entry document.",
            "Review the selected document root and re-import a valid static build."));
    }
    issues.truncate(8);
    let has_actionable_findings=!issues.is_empty();
    Report{issues,lines_reviewed:logs.len(),has_actionable_findings,engine:"offline-rules-v1"}
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn diagnosis_is_specific_and_never_echoes_credentials(){
        let canary="do-not-expose-secret-123";
        let logs=vec![
            format!("password={canary} TypeError: Cannot read properties of undefined (reading 'prototype')"),
            "at node_modules/buffer-equal-constant-time/index.js Node.js v26.13.0".into(),
            format!("Access denied for user 'root' password={canary}")
        ];
        let report=analyze(&logs,"node","error");
        let payload=serde_json::to_string(&report).unwrap();
        assert!(payload.contains("NODE26_LEGACY_DEP"));
        assert!(payload.contains("MYSQL_AUTH"));
        assert!(!payload.contains(canary));
        assert!(report.issues.iter().all(|i|!i.auto_fixed));
    }
    #[test]
    fn unknown_errors_fail_closed_without_suggesting_blind_restart(){
        let report=analyze(&["unexpected internal error".into()],"php","error");
        assert_eq!(report.issues[0].id,"UNKNOWN_FAILURE");
        assert!(!report.has_actionable_findings==false);
    }
}
