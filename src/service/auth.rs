//! `AuthService` (`/AS4`): token refresh, logout and auth-key management.

use super::AuthService;
use crate::base::ClientEvent;
use crate::error::{Error, Result};
use crate::request::{Parse, RequestOptions};
use crate::thrift::{self, Protocol, Schema, TValue, Value};

/// Pre-encoded compact `logoutZ` call with empty args.
const LOGOUT_Z_PAYLOAD: [u8; 12] = [0x82, 0x21, 0x00, 0x07, b'l', b'o', b'g', b'o', b'u', b't', b'Z', 0x00];
const LOGOUT_Z_PATH: &str = "/RS4";

impl AuthService<'_> {
    /// Refresh the v3 access token with the stored refresh token (see
    /// [`BaseClient::try_refresh_token`](crate::BaseClient::try_refresh_token)).
    pub async fn try_refresh_token(&self) -> Result<()> {
        self.client.try_refresh_token().await
    }

    /// Log out this device (`logoutZ` on `/RS4`) and disable the client:
    /// later requests fail with `ClientClosed`, `End` is emitted and the auth
    /// token is dropped. Returns the decoded `success` (or `true`).
    pub async fn logout_z(&self) -> Result<Value> {
        let client = self.client;
        let res = client
            .send_http(
                LOGOUT_Z_PATH,
                "POST",
                client.line_headers("POST"),
                LOGOUT_Z_PAYLOAD.to_vec(),
                client.config.timeout,
            )
            .await?;
        if let Some(next) = res.header("x-line-next-access") {
            client.emit(ClientEvent::UpdateAuthToken(next.to_string()));
        }
        let msg = thrift::read_message(&res.body, Protocol::Compact).map_err(|_| {
            Error::internal(
                "RequestError",
                format!("Invalid response buffer for logoutZ: <{}>", hex::encode(&res.body)),
            )
        })?;
        let TValue::Struct(fields) = &msg.body else { unreachable!("read_message returns a struct") };
        if let Some((_, exc)) = fields.iter().find(|(id, _)| *id != 0) {
            let data = thrift::decode_struct(Schema::line(), Some("TalkException"), exc);
            let refreshable = data["code"].as_str() == Some("MUST_REFRESH_V3_TOKEN")
                && client.storage.get("refreshToken").await?.is_some();
            if !refreshable {
                return Err(Error::Request { method: "logoutZ".into(), path: LOGOUT_Z_PATH.into(), data });
            }
        }
        let result = msg.body.field(0).map(thrift::decode_raw).unwrap_or(Value::Bool(true));
        client.disable();
        client.stop_listening();
        if let Some(profile) = client.profile() {
            client.emit(ClientEvent::End(Some(profile)));
        }
        client.set_auth_token(None);
        Ok(result)
    }
}

impl AuthService<'_> {
    /// Log out the current session (v2; no arguments).
    ///
    /// Recovered by static RE of LINE macOS 26.5.0; unverified against a live
    /// server.
    pub async fn logout_v2(&self) -> Result<Value> {
        self.request("logoutV2", Vec::new(), Parse::Result).await
    }
}

// <generated pass-through>
impl AuthService<'_> {
    /// `confirmIdentifier(authSessionId: string, request: IdentityCredentialRequest)`.
    pub async fn confirm_identifier(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("confirmIdentifier", args).await
    }

    /// `connectEapAccount(request: ConnectEapAccountRequest)`.
    pub async fn connect_eap_account(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("connectEapAccount", args).await
    }

    /// `disconnectEapAccount(request: DisconnectEapAccountRequest)`.
    pub async fn disconnect_eap_account(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("disconnectEapAccount", args).await
    }

    /// `establishE2EESession(request: YN0_Ob1_J)`.
    pub async fn establish_e2ee_session(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("establishE2EESession", args).await
    }

    /// `getAuthRSAKey(authSessionId: string, identityProvider: IdentityProvider)`.
    pub async fn get_auth_rsa_key(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getAuthRSAKey", args).await
    }

    /// `issueTokenForAccountMigrationSettings(enforce: bool)`.
    pub async fn issue_token_for_account_migration_settings(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("issueTokenForAccountMigrationSettings", args).await
    }

    /// `issueV3TokenForPrimary(request: IssueV3TokenForPrimaryRequest)`.
    pub async fn issue_v3_token_for_primary(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("issueV3TokenForPrimary", args).await
    }

    /// `openAuthSession(request: AuthSessionRequest)`.
    pub async fn open_auth_session(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("openAuthSession", args).await
    }

    /// `openSession(request: OpenSessionRequest)`.
    pub async fn open_session(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("openSession", args).await
    }

    /// `refresh(request: RefreshAccessTokenRequest)`.
    pub async fn refresh(&self, args: impl Into<Value>) -> Result<Value> {
        self.call_with("refresh", args, |o| RequestOptions { path: "/EXT/auth/tokenrefresh/v1", ..o }).await
    }

    /// `removeIdentifier(authSessionId: string, request: IdentityCredentialRequest)`.
    pub async fn remove_identifier(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("removeIdentifier", args).await
    }

    /// `reportRefreshedAccessToken(request: ReportRefreshedAccessTokenRequest)`.
    pub async fn report_refreshed_access_token(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("reportRefreshedAccessToken", args).await
    }

    /// `resendIdentifierConfirmation(authSessionId: string, request: IdentityCredentialRequest)`.
    pub async fn resend_identifier_confirmation(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("resendIdentifierConfirmation", args).await
    }

    /// `setIdentifier(authSessionId: string, request: IdentityCredentialRequest)`.
    pub async fn set_identifier(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("setIdentifier", args).await
    }

    /// `updateIdentifier(authSessionId: string, request: IdentityCredentialRequest)`.
    pub async fn update_identifier(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("updateIdentifier", args).await
    }

    /// `updatePassword(request: UpdatePasswordRequest)`.
    pub async fn update_password(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("updatePassword", args).await
    }

    /// `verifyEapLogin(request: VerifyEapLoginRequest)`.
    pub async fn verify_eap_login(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("verifyEapLogin", args).await
    }
}
// </generated pass-through>

#[cfg(test)]
mod tests {
    use crate::service::test_util::{encode, path, s};
    use crate::thrift::{Protocol, read_message};

    #[test]
    fn logout_z_payload_is_an_empty_compact_call() {
        let msg = read_message(&super::LOGOUT_Z_PAYLOAD, Protocol::Compact).unwrap();
        assert_eq!(msg.name, "logoutZ");
        assert_eq!(msg.seqid, 0);
    }

    #[test]
    fn refresh_args_encoding() {
        let f = encode("refresh_args", &crate::tv!({"request": {"refreshToken": "rt-old"}}));
        assert_eq!(path(&f, &[1, 1]), Some(&s("rt-old")));
    }

    #[tokio::test]
    async fn try_refresh_token_requires_a_stored_token() {
        let c = crate::service::test_util::client();
        let err = c.auth().try_refresh_token().await.unwrap_err();
        assert!(err.to_string().contains("refreshToken not found"), "{err}");
    }
}
