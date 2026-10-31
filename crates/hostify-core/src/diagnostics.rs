//! Deterministic, fail-closed publish diagnosis. Classification does not perform repairs.
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Class {
    TransientRetryable,
    AutoFixSafe,
    UserApprovalRequired,
    ConfigurationError,
    ApplicationError,
    SecurityBlocker,
    DataIntegrityBlocker,
    ExternalProviderError,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub class: Class,
    pub message: String,
    pub next_action: String,
    /// Only bounded, non-mutating attempts are eligible for automatic retry.
    pub safe_to_retry: bool,
}

pub fn classify(error: &str) -> Diagnostic {
    let e = error.to_ascii_lowercase();
    let (class, next, retry) =
        if e.contains("another website") || e.contains("schema") || e.contains("rollback incomplete") || e.contains("ownership") {
            (Class::DataIntegrityBlocker, "Preserve the current database and deployment. Resolve the ownership or data issue explicitly.", false)
        } else if e.contains("421") || e.contains("foreign host") || e.contains("host not allowed") || e.contains("exposed") || e.contains("local system") || e.contains("localsystem") {
            (Class::SecurityBlocker, "Inspect the host routing and process permissions; never disable security checks.", false)
        } else if e.contains("trust") || e.contains("acknowledge") || e.contains("node.js 26") || e.contains("runtime") && (e.contains("missing") || e.contains("choose")) {
            (Class::UserApprovalRequired, "Review the compatibility and trust requirements, then explicitly approve changes.", false)
        } else if e.contains("cloudflare") || e.contains("dns") && (e.contains("permission") || e.contains("api")) {
            (Class::ExternalProviderError, "Check provider credentials and connectivity. Do not repeat DNS writes blindly.", false)
        } else if e.contains("dns") && (e.contains("propagation") || e.contains("not visible")) || e.contains("tls pending") {
            (Class::TransientRetryable, "Wait for propagation and recheck without changing DNS resources.", true)
        } else if e.contains("port unavailable") || e.contains("port in use") || e.contains("no available local port") {
            (Class::AutoFixSafe, "Allocate a different unused loopback port before binding; never terminate unrelated listeners.", false)
        } else if e.contains("timeout") || e.contains("timed out") || e.contains("temporarily unavailable") || e.contains("connection reset") {
            (Class::TransientRetryable, "Retry the read-only check with bounded exponential backoff. Never repeat provisioning writes.", true)
        } else if e.contains("exit code") || e.contains("exited with") || e.contains("500") || e.contains("module not found") {
            (Class::ApplicationError, "Inspect application logs and dependencies; rebuild only with the owner's approval.", false)
        } else {
            (Class::ConfigurationError, "Review the failed step and its logs before retrying.", false)
        };
    Diagnostic {
        class,
        message: error.chars().take(1024).collect(),
        next_action: next.to_string(),
        safe_to_retry: retry,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unsafe_errors_never_auto_retry() {
        for message in ["HTTP 421 Host not allowed", "Database belongs to another website",
            "Node.js 26 dependency incompatible", "Application exited with code 1",
            "Cloudflare API permission denied", "rollback incomplete"] {
            assert!(!classify(message).safe_to_retry, "{message}");
        }
    }
    #[test]
    fn only_bounded_read_only_rechecks() {
        let d = classify("DNS propagation not visible");
        assert_eq!(d.class, Class::TransientRetryable);
        assert!(d.safe_to_retry);
        assert_eq!(classify("No available local port").class, Class::AutoFixSafe);
    }
}
