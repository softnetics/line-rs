//! `PasskeyAccountMigrationService`: move an account to a new primary device
//! by signing in with a passkey.
//!
//! Path: `/ext/auth/feature-guest/thrift/account/v1/migrate/passkey`.
//! Recovered by static RE of LINE Android 26.x; unverified against a live
//! server. Flow: [`open_session`](PasskeyAccountMigrationService::open_session),
//! [`get_passkey_authentication_options`](PasskeyAccountMigrationService::get_passkey_authentication_options)
//! (hand `credentialRequestOptions.publicKey` to an authenticator),
//! [`authenticate_passkey`](PasskeyAccountMigrationService::authenticate_passkey)
//! with its assertion, then
//! [`migrate_primary_using_passkey`](PasskeyAccountMigrationService::migrate_primary_using_passkey),
//! which issues the new device's tokens.

use super::PasskeyAccountMigrationService;
use crate::error::{Error, Result};
use crate::request::Parse;
use crate::thrift::{TValue, Value};

impl PasskeyAccountMigrationService<'_> {
    /// Open a migration session for this device. Returns its `sessionId`.
    ///
    /// The IDL's `openSession_args` belongs to other services (with a
    /// different `OpenSessionRequest`), so this one is encoded by hand:
    /// `OpenSessionRequest { 1: Device { 1: deviceModel, 2: deviceId } }`.
    pub async fn open_session(&self, device_model: &str, device_id: &str) -> Result<String> {
        let device = TValue::Struct(vec![
            (1, TValue::string(device_model)),
            (2, TValue::string(device_id)),
        ]);
        let args = vec![(1, TValue::Struct(vec![(1, device)]))];
        let res = self.request("openSession", args, Parse::Raw).await?;
        res.get("1")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| Error::internal("PasskeyError", "no sessionId in response"))
    }

    /// WebAuthn request options for the session
    /// (`{credentialRequestOptions: {mediation, publicKey}}`).
    pub async fn get_passkey_authentication_options(&self, session_id: &str) -> Result<Value> {
        self.call(
            "getPasskeyAuthenticationOptions",
            crate::tv!({"request": {"sessionId": session_id}}),
        )
        .await
    }

    /// Check the authenticator's assertion. `credential` is an
    /// `AuthenticationPublicKeyCredential` (`{id, type, response:
    /// {clientDataJson, authenticatorData, signature, userHandle},
    /// authenticatorAttachment, clientExtensionResults}`). Returns
    /// `{sameUdidFromAccount}`.
    pub async fn authenticate_passkey(
        &self,
        session_id: &str,
        credential: impl Into<Value>,
    ) -> Result<Value> {
        let mut req = crate::tv!({"sessionId": session_id});
        req.insert("authenticationPublicKeyCredential", credential.into());
        let mut args = Value::object();
        args.insert("request", req);
        self.call("authenticatePasskey", args).await
    }

    /// Finish the migration. Returns the account `mid` and the new device's
    /// `tokenV3IssueResult` / `tokenV1IssueResult`.
    pub async fn migrate_primary_using_passkey(&self, session_id: &str) -> Result<Value> {
        self.call(
            "migratePrimaryUsingPasskey",
            crate::tv!({"request": {"sessionId": session_id}}),
        )
        .await
    }
}
