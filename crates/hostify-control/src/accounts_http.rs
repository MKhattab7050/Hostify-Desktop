//! Local-only tenant account endpoints for the trusted development control
//! plane. This is not an Internet-facing identity service: TLS, MFA, rate
//! limiting, and tenant gateways must be independently implemented first.
use hostify_core::network_auth::{AccountError, Accounts, Capability, Principal, Role};
use hyper::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Mutex;

pub struct AccountApi {
    accounts: Mutex<Accounts>,
}
impl AccountApi {
    pub fn new(accounts: Accounts) -> Self {
        Self {
            accounts: Mutex::new(accounts),
        }
    }

    pub fn authorize(
        &self,
        auth: Option<&str>,
        tenant: &str,
        cap: Capability,
    ) -> Result<Principal, (StatusCode, &'static str)> {
        let token = auth
            .and_then(|h| h.strip_prefix("Bearer "))
            .ok_or((StatusCode::UNAUTHORIZED, "unauthorized"))?;
        let accounts = self
            .accounts
            .lock()
            .map_err(|_| (StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?;
        accounts
            .authorize(token, tenant, cap)
            .map_err(map_account_error)
    }

    pub fn dispatch(
        &self,
        path: &str,
        auth: Option<&str>,
        admin: bool,
        body: &[u8],
    ) -> (StatusCode, Value) {
        fn parse<T: serde::de::DeserializeOwned>(
            body: &[u8],
        ) -> Result<T, (StatusCode, &'static str)> {
            serde_json::from_slice(body).map_err(|_| (StatusCode::BAD_REQUEST, "invalid_json"))
        }
        fn session_token(auth: Option<&str>) -> Result<&str, (StatusCode, &'static str)> {
            auth.and_then(|header| header.strip_prefix("Bearer "))
                .ok_or((StatusCode::UNAUTHORIZED, "unauthorized"))
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Provision {
            name: String,
            email: String,
            password: String,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Bootstrap {
            tenant_id: String,
            email: String,
            password: String,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Login {
            tenant_id: String,
            email: String,
            password: String,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Me {
            tenant_id: String,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Invite {
            tenant_id: String,
            email: String,
            role: Role,
            ttl_seconds: i64,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct InvitationList {
            tenant_id: String,
            limit: i64,
            offset: i64,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct InvitationRevoke {
            tenant_id: String,
            invitation_id: String,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Accept {
            invitation: String,
            password: String,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Disable {
            tenant_id: String,
            user_id: String,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct RoleUpdate {
            tenant_id: String,
            user_id: String,
            role: Role,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct ListMembers {
            tenant_id: String,
            limit: i64,
            offset: i64,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct PasswordChange {
            tenant_id: String,
            current_password: String,
            new_password: String,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct SessionList {
            tenant_id: String,
            limit: i64,
            offset: i64,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct SessionRevoke {
            tenant_id: String,
            session_id: String,
        }
        let response: Result<Value, (StatusCode, &'static str)> = (|| {
            let mut accounts = self
                .accounts
                .lock()
                .map_err(|_| (StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?;
            match path {
                "/v1/accounts/provision" => {
                    if !admin {
                        return Err((StatusCode::UNAUTHORIZED, "unauthorized"));
                    }
                    let p: Provision = parse(body)?;
                    let (tenant, owner) = accounts
                        .provision_owner_tenant(&p.name, &p.email, &p.password)
                        .map_err(map_account_error)?;
                    Ok(json!({"tenant":tenant,"owner":owner}))
                }
                "/v1/accounts/bootstrap" => {
                    if !admin {
                        return Err((StatusCode::UNAUTHORIZED, "unauthorized"));
                    }
                    let p: Bootstrap = parse(body)?;
                    let principal = accounts
                        .bootstrap_owner(&p.tenant_id, &p.email, &p.password)
                        .map_err(map_account_error)?;
                    Ok(json!({"owner":principal}))
                }
                "/v1/auth/login" => {
                    let p: Login = parse(body)?;
                    let session = accounts
                        .sign_in(&p.tenant_id, &p.email, &p.password)
                        .map_err(map_account_error)?;
                    Ok(
                        json!({"token":session.token,"expiresUnix":session.expires_unix,
                        "user":session.principal}),
                    )
                }
                "/v1/auth/logout" => {
                    accounts
                        .sign_out(session_token(auth)?)
                        .map_err(map_account_error)?;
                    Ok(json!({"signedOut":true}))
                }
                "/v1/auth/sessions/list" => {
                    let p: SessionList = parse(body)?;
                    let items = accounts
                        .list_sessions(session_token(auth)?, &p.tenant_id, p.limit, p.offset)
                        .map_err(map_account_error)?;
                    Ok(json!({"items":items,"limit":p.limit,"offset":p.offset}))
                }
                "/v1/auth/sessions/revoke" => {
                    let p: SessionRevoke = parse(body)?;
                    accounts
                        .revoke_session(session_token(auth)?, &p.tenant_id, &p.session_id)
                        .map_err(map_account_error)?;
                    Ok(json!({"revoked":true}))
                }
                "/v1/auth/password/change" => {
                    let p: PasswordChange = parse(body)?;
                    accounts
                        .change_password(
                            session_token(auth)?,
                            &p.tenant_id,
                            &p.current_password,
                            &p.new_password,
                        )
                        .map_err(map_account_error)?;
                    Ok(json!({"changed":true,"sessionsRevoked":true,"requiresLogin":true}))
                }
                "/v1/auth/me" => {
                    let p: Me = parse(body)?;
                    let principal = accounts
                        .authorize(session_token(auth)?, &p.tenant_id, Capability::View)
                        .map_err(map_account_error)?;
                    Ok(json!({"user":principal}))
                }
                "/v1/members/invite" => {
                    let p: Invite = parse(body)?;
                    let invitation = accounts
                        .invite_member(
                            session_token(auth)?,
                            &p.tenant_id,
                            &p.email,
                            p.role,
                            p.ttl_seconds,
                        )
                        .map_err(map_account_error)?;
                    Ok(json!({"invitation":invitation.token,
                        "invitationId":invitation.id,"expiresUnix":invitation.expires_unix}))
                }
                "/v1/members/invitations/list" => {
                    let p: InvitationList = parse(body)?;
                    let items = accounts
                        .list_pending_invitations(
                            session_token(auth)?,
                            &p.tenant_id,
                            p.limit,
                            p.offset,
                        )
                        .map_err(map_account_error)?;
                    Ok(json!({"items":items,"limit":p.limit,"offset":p.offset}))
                }
                "/v1/members/invitations/revoke" => {
                    let p: InvitationRevoke = parse(body)?;
                    accounts
                        .revoke_member_invitation(
                            session_token(auth)?,
                            &p.tenant_id,
                            &p.invitation_id,
                        )
                        .map_err(map_account_error)?;
                    Ok(json!({"revoked":true}))
                }
                "/v1/members/list" => {
                    let p: ListMembers = parse(body)?;
                    let items = accounts
                        .list_members(session_token(auth)?, &p.tenant_id, p.limit, p.offset)
                        .map_err(map_account_error)?;
                    Ok(json!({"items":items,"limit":p.limit,"offset":p.offset}))
                }
                "/v1/members/role/update" => {
                    let p: RoleUpdate = parse(body)?;
                    accounts
                        .update_member_role(session_token(auth)?, &p.tenant_id, &p.user_id, p.role)
                        .map_err(map_account_error)?;
                    Ok(json!({"accepted":true}))
                }
                "/v1/members/disable" => {
                    let p: Disable = parse(body)?;
                    accounts
                        .disable_member(session_token(auth)?, &p.tenant_id, &p.user_id)
                        .map_err(map_account_error)?;
                    Ok(json!({"disabled":true}))
                }
                "/v1/members/accept" => {
                    let p: Accept = parse(body)?;
                    let member = accounts
                        .accept_invitation(&p.invitation, &p.password)
                        .map_err(map_account_error)?;
                    Ok(json!({"member":member}))
                }
                _ => Err((StatusCode::NOT_FOUND, "not_found")),
            }
        })();
        match response {
            Ok(value) => (StatusCode::OK, value),
            Err((status, message)) => (status, json!({"error":message})),
        }
    }
}

pub fn is_account_path(path: &str) -> bool {
    matches!(
        path,
        "/v1/accounts/provision"
            | "/v1/accounts/bootstrap"
            | "/v1/auth/login"
            | "/v1/auth/logout"
            | "/v1/auth/me"
            | "/v1/auth/password/change"
            | "/v1/auth/sessions/list"
            | "/v1/auth/sessions/revoke"
            | "/v1/members/invite"
            | "/v1/members/accept"
            | "/v1/members/disable"
            | "/v1/members/list"
            | "/v1/members/role/update"
            | "/v1/members/invitations/list"
            | "/v1/members/invitations/revoke"
    )
}
fn map_account_error(error: AccountError) -> (StatusCode, &'static str) {
    match error {
        AccountError::Invalid => (StatusCode::BAD_REQUEST, "invalid_request"),
        AccountError::Authentication | AccountError::Invitation | AccountError::Unavailable => {
            (StatusCode::UNAUTHORIZED, "unauthorized")
        }
        AccountError::Locked => (StatusCode::TOO_MANY_REQUESTS, "temporarily_locked"),
        AccountError::Forbidden => (StatusCode::FORBIDDEN, "not_authorized"),
        AccountError::Database(_) | AccountError::Random => {
            (StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hostify_core::network::Registry;
    use uuid::Uuid;

    #[test]
    fn local_member_disable_requires_role_and_revokes_existing_sessions() {
        let path =
            std::env::temp_dir().join(format!("hostify-member-disable-{}.db", Uuid::new_v4()));
        let mut registry = Registry::open(&path, "example.test").unwrap();
        let tenant = registry.create_tenant("A").unwrap();
        let other = registry.create_tenant("B").unwrap();
        let api = AccountApi::new(Accounts::open(&path).unwrap());
        let post = |path: &str, auth: Option<&str>, admin: bool, value: Value| {
            api.dispatch(path, auth, admin, value.to_string().as_bytes())
        };
        let (status, created) = post(
            "/v1/accounts/bootstrap",
            None,
            true,
            json!({
                "tenantId":tenant.id,"email":"owner@example.com","password":"long-owner-password"
            }),
        );
        assert_eq!(status, StatusCode::OK);
        assert_eq!(created["owner"]["role"], "owner");
        let (status, login) = post(
            "/v1/auth/login",
            None,
            false,
            json!({
                "tenantId":tenant.id,"email":"owner@example.com","password":"long-owner-password"
            }),
        );
        assert_eq!(status, StatusCode::OK);
        let owner_auth = format!("Bearer {}", login["token"].as_str().unwrap());
        let (status, invite) = post(
            "/v1/members/invite",
            Some(&owner_auth),
            false,
            json!({
                "tenantId":tenant.id,"email":"viewer@example.com","role":"viewer","ttlSeconds":600
            }),
        );
        assert_eq!(status, StatusCode::OK);
        let (status, member) = post(
            "/v1/members/accept",
            None,
            false,
            json!({
                "invitation":invite["invitation"],"password":"long-viewer-password"
            }),
        );
        assert_eq!(status, StatusCode::OK);
        let user_id = member["member"]["userId"].as_str().unwrap();
        let (status, viewer) = post(
            "/v1/auth/login",
            None,
            false,
            json!({
                "tenantId":tenant.id,"email":"viewer@example.com","password":"long-viewer-password"
            }),
        );
        assert_eq!(status, StatusCode::OK);
        let viewer_auth = format!("Bearer {}", viewer["token"].as_str().unwrap());
        let payload = json!({"tenantId":tenant.id,"userId":user_id});
        assert_eq!(
            post(
                "/v1/members/disable",
                Some(&viewer_auth),
                false,
                payload.clone()
            )
            .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            post(
                "/v1/members/disable",
                Some(&owner_auth),
                false,
                json!({
                    "tenantId":other.id,"userId":user_id
                })
            )
            .0,
            StatusCode::UNAUTHORIZED
        );
        let (status, disabled) = post("/v1/members/disable", Some(&owner_auth), false, payload);
        assert_eq!(status, StatusCode::OK);
        assert_eq!(disabled["disabled"], true);
        assert_eq!(
            post(
                "/v1/auth/me",
                Some(&viewer_auth),
                false,
                json!({"tenantId":tenant.id})
            )
            .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(post("/v1/auth/login",None,false,json!({
            "tenantId":tenant.id,"email":"viewer@example.com","password":"long-viewer-password"
        })).0,StatusCode::UNAUTHORIZED);
        assert_eq!(
            post(
                "/v1/auth/me",
                Some(&owner_auth),
                false,
                json!({"tenantId":tenant.id})
            )
            .0,
            StatusCode::OK
        );
        drop(post);
        drop(api);
        drop(registry);
        std::fs::remove_file(&path).unwrap();
    }
    #[test]
    fn member_inventory_http_requires_management_session_and_scopes_result() {
        let path = std::env::temp_dir().join(format!("hostify-members-list-{}.db", Uuid::new_v4()));
        let mut registry = Registry::open(&path, "example.test").unwrap();
        let a = registry.create_tenant("A").unwrap();
        let b = registry.create_tenant("B").unwrap();
        let api = AccountApi::new(Accounts::open(&path).unwrap());
        let post = |path: &str, auth: Option<&str>, admin: bool, value: Value| {
            api.dispatch(path, auth, admin, value.to_string().as_bytes())
        };
        assert_eq!(
            post(
                "/v1/accounts/bootstrap",
                None,
                true,
                json!({
                    "tenantId":a.id,"email":"owner@site.test","password":"valid-owner-password"
                })
            )
            .0,
            StatusCode::OK
        );
        let (_, login) = post(
            "/v1/auth/login",
            None,
            false,
            json!({
                "tenantId":a.id,"email":"owner@site.test","password":"valid-owner-password"
            }),
        );
        let bearer = format!("Bearer {}", login["token"].as_str().unwrap());
        let payload = json!({"tenantId":a.id,"limit":10,"offset":0});
        assert_eq!(
            post("/v1/members/list", None, false, payload.clone()).0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            post(
                "/v1/members/list",
                Some(&bearer),
                false,
                json!({
                    "tenantId":b.id,"limit":10,"offset":0
                })
            )
            .0,
            StatusCode::UNAUTHORIZED
        );
        let (status, data) = post("/v1/members/list", Some(&bearer), false, payload);
        assert_eq!(status, StatusCode::OK);
        assert_eq!(data["items"].as_array().unwrap().len(), 1);
        assert_eq!(data["items"][0]["role"], "owner");
        assert!(data["items"][0].get("passwordHash").is_none());
        assert_eq!(
            post(
                "/v1/members/list",
                Some(&bearer),
                false,
                json!({
                    "tenantId":a.id,"limit":101,"offset":0
                })
            )
            .0,
            StatusCode::BAD_REQUEST
        );
        drop(post);
        drop(api);
        drop(registry);
        std::fs::remove_file(&path).unwrap();
    }
    #[test]
    fn password_change_http_requires_current_password_and_relogin() {
        let path =
            std::env::temp_dir().join(format!("hostify-password-change-{}.db", Uuid::new_v4()));
        let mut registry = Registry::open(&path, "example.test").unwrap();
        let tenant = registry.create_tenant("A").unwrap();
        let api = AccountApi::new(Accounts::open(&path).unwrap());
        let post = |path: &str, auth: Option<&str>, admin: bool, value: Value| {
            api.dispatch(path, auth, admin, value.to_string().as_bytes())
        };
        assert_eq!(post("/v1/accounts/bootstrap",None,true,json!({
            "tenantId":tenant.id,"email":"owner@site.test","password":"initial-owner-password"
        })).0,StatusCode::OK);
        let (_, login) = post(
            "/v1/auth/login",
            None,
            false,
            json!({
                "tenantId":tenant.id,"email":"owner@site.test","password":"initial-owner-password"
            }),
        );
        let auth = format!("Bearer {}", login["token"].as_str().unwrap());
        let payload = json!({"tenantId":tenant.id,
            "currentPassword":"initial-owner-password","newPassword":"replacement-owner-password"});
        assert_eq!(
            post("/v1/auth/password/change", None, false, payload.clone()).0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            post(
                "/v1/auth/password/change",
                Some(&auth),
                false,
                json!({
                    "tenantId":tenant.id,"currentPassword":"wrong-password",
                    "newPassword":"replacement-owner-password"
                })
            )
            .0,
            StatusCode::UNAUTHORIZED
        );
        let (status, result) = post("/v1/auth/password/change", Some(&auth), false, payload);
        assert_eq!(status, StatusCode::OK);
        assert_eq!(result["sessionsRevoked"], true);
        assert_eq!(result["requiresLogin"], true);
        assert_eq!(
            post(
                "/v1/auth/me",
                Some(&auth),
                false,
                json!({"tenantId":tenant.id})
            )
            .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            post(
                "/v1/auth/login",
                None,
                false,
                json!({
                    "tenantId":tenant.id,"email":"owner@site.test",
                    "password":"replacement-owner-password"
                })
            )
            .0,
            StatusCode::OK
        );
        drop(post);
        drop(api);
        drop(registry);
        std::fs::remove_file(&path).unwrap();
    }
    #[test]
    fn sessions_http_returns_no_tokens_and_supports_own_session_revocation() {
        let path = std::env::temp_dir().join(format!("hostify-sessions-{}.db", Uuid::new_v4()));
        let mut registry = Registry::open(&path, "example.test").unwrap();
        let tenant = registry.create_tenant("A").unwrap();
        let api = AccountApi::new(Accounts::open(&path).unwrap());
        let post = |path: &str, auth: Option<&str>, admin: bool, value: Value| {
            api.dispatch(path, auth, admin, value.to_string().as_bytes())
        };
        assert_eq!(
            post(
                "/v1/accounts/bootstrap",
                None,
                true,
                json!({
                    "tenantId":tenant.id,"email":"owner@site.test","password":"valid-owner-password"
                })
            )
            .0,
            StatusCode::OK
        );
        let (_, login) = post(
            "/v1/auth/login",
            None,
            false,
            json!({
                "tenantId":tenant.id,"email":"owner@site.test","password":"valid-owner-password"
            }),
        );
        let auth = format!("Bearer {}", login["token"].as_str().unwrap());
        let (status, data) = post(
            "/v1/auth/sessions/list",
            Some(&auth),
            false,
            json!({"tenantId":tenant.id,"limit":10,"offset":0}),
        );
        assert_eq!(status, StatusCode::OK);
        assert_eq!(data["items"].as_array().unwrap().len(), 1);
        assert_eq!(data["items"][0]["isCurrent"], true);
        assert!(data["items"][0].get("token").is_none());
        assert!(data["items"][0].get("tokenHash").is_none());
        let sid = data["items"][0]["sessionId"].as_str().unwrap();
        assert_eq!(
            post(
                "/v1/auth/sessions/revoke",
                None,
                false,
                json!({"tenantId":tenant.id,"sessionId":sid})
            )
            .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            post(
                "/v1/auth/sessions/revoke",
                Some(&auth),
                false,
                json!({"tenantId":tenant.id,"sessionId":sid})
            )
            .0,
            StatusCode::OK
        );
        assert_eq!(
            post(
                "/v1/auth/sessions/list",
                Some(&auth),
                false,
                json!({"tenantId":tenant.id,"limit":10,"offset":0})
            )
            .0,
            StatusCode::UNAUTHORIZED
        );
        drop(post);
        drop(api);
        drop(registry);
        std::fs::remove_file(&path).unwrap();
    }
    #[test]
    fn role_update_http_rejects_non_owner_and_returns_tenant_local_result() {
        let path = std::env::temp_dir().join(format!("hostify-member-role-{}.db", Uuid::new_v4()));
        let mut registry = Registry::open(&path, "example.test").unwrap();
        let tenant = registry.create_tenant("A").unwrap();
        let api = AccountApi::new(Accounts::open(&path).unwrap());
        let post = |path: &str, auth: Option<&str>, admin: bool, value: Value| {
            api.dispatch(path, auth, admin, value.to_string().as_bytes())
        };
        assert_eq!(
            post(
                "/v1/accounts/bootstrap",
                None,
                true,
                json!({
                    "tenantId":tenant.id,"email":"owner@site.test","password":"valid-owner-password"
                })
            )
            .0,
            StatusCode::OK
        );
        let (_, owner) = post(
            "/v1/auth/login",
            None,
            false,
            json!({
                "tenantId":tenant.id,"email":"owner@site.test","password":"valid-owner-password"
            }),
        );
        let owner_auth = format!("Bearer {}", owner["token"].as_str().unwrap());
        let (_, invite) = post(
            "/v1/members/invite",
            Some(&owner_auth),
            false,
            json!({
                "tenantId":tenant.id,"email":"admin@site.test","role":"admin","ttlSeconds":600
            }),
        );
        let (_, member) = post(
            "/v1/members/accept",
            None,
            false,
            json!({
                "invitation":invite["invitation"],"password":"valid-admin-password"
            }),
        );
        let user_id = member["member"]["userId"].as_str().unwrap();
        let (_, admin) = post(
            "/v1/auth/login",
            None,
            false,
            json!({
                "tenantId":tenant.id,"email":"admin@site.test","password":"valid-admin-password"
            }),
        );
        let admin_auth = format!("Bearer {}", admin["token"].as_str().unwrap());
        let payload = json!({"tenantId":tenant.id,"userId":user_id,"role":"viewer"});
        assert_eq!(
            post(
                "/v1/members/role/update",
                Some(&admin_auth),
                false,
                payload.clone()
            )
            .0,
            StatusCode::FORBIDDEN
        );
        let (status, changed) = post("/v1/members/role/update", Some(&owner_auth), false, payload);
        assert_eq!(status, StatusCode::OK);
        assert_eq!(changed["accepted"], true);
        assert_eq!(
            post(
                "/v1/auth/me",
                Some(&admin_auth),
                false,
                json!({"tenantId":tenant.id})
            )
            .0,
            StatusCode::UNAUTHORIZED
        );
        drop(post);
        drop(api);
        drop(registry);
        std::fs::remove_file(&path).unwrap();
    }
    #[test]
    fn pending_invitation_http_list_and_revocation_do_not_expose_secret() {
        let path =
            std::env::temp_dir().join(format!("hostify-pending-invites-{}.db", Uuid::new_v4()));
        let mut registry = Registry::open(&path, "example.test").unwrap();
        let a = registry.create_tenant("A").unwrap();
        let b = registry.create_tenant("B").unwrap();
        let api = AccountApi::new(Accounts::open(&path).unwrap());
        let post = |path: &str, auth: Option<&str>, admin: bool, value: Value| {
            api.dispatch(path, auth, admin, value.to_string().as_bytes())
        };
        assert_eq!(
            post(
                "/v1/accounts/bootstrap",
                None,
                true,
                json!({
                    "tenantId":a.id,"email":"owner@site.test","password":"owner-valid-password"
                })
            )
            .0,
            StatusCode::OK
        );
        let (_, owner) = post(
            "/v1/auth/login",
            None,
            false,
            json!({
                "tenantId":a.id,"email":"owner@site.test","password":"owner-valid-password"
            }),
        );
        let auth = format!("Bearer {}", owner["token"].as_str().unwrap());
        let (_, issued) = post(
            "/v1/members/invite",
            Some(&auth),
            false,
            json!({
                "tenantId":a.id,"email":"viewer@site.test","role":"viewer","ttlSeconds":600
            }),
        );
        let payload = json!({"tenantId":a.id,"limit":10,"offset":0});
        assert_eq!(
            post("/v1/members/invitations/list", None, false, payload.clone()).0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            post(
                "/v1/members/invitations/list",
                Some(&auth),
                false,
                json!({"tenantId":b.id,"limit":10,"offset":0})
            )
            .0,
            StatusCode::UNAUTHORIZED
        );
        let (status, pending) = post(
            "/v1/members/invitations/list",
            Some(&auth),
            false,
            payload.clone(),
        );
        assert_eq!(status, StatusCode::OK);
        assert_eq!(pending["items"][0]["id"], issued["invitationId"]);
        assert!(pending["items"][0].get("tokenHash").is_none());
        assert!(
            !pending
                .to_string()
                .contains(issued["invitation"].as_str().unwrap())
        );
        assert_eq!(
            post(
                "/v1/members/invitations/revoke",
                Some(&auth),
                false,
                json!({
                    "tenantId":a.id,"invitationId":issued["invitationId"]
                })
            )
            .0,
            StatusCode::OK
        );
        let (_, after) = post("/v1/members/invitations/list", Some(&auth), false, payload);
        assert!(after["items"].as_array().unwrap().is_empty());
        assert_eq!(
            post(
                "/v1/members/accept",
                None,
                false,
                json!({
                    "invitation":issued["invitation"],"password":"viewer-valid-password"
                })
            )
            .0,
            StatusCode::UNAUTHORIZED
        );
        drop(post);
        drop(api);
        drop(registry);
        std::fs::remove_file(&path).unwrap();
    }
}
