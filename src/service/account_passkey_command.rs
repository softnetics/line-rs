//! `AccountPasskeyCommandService`: register, rename and delete the account's
//! passkeys (WebAuthn credentials).
//!
//! Path: `/EXT/auth/feature-user/thrift/account/passkey/v1/command`.
//! Recovered by static RE of LINE Android 26.x; unverified against a live
//! server. Registration is a three-step WebAuthn ceremony:
//! [`open_session_for_passkey_registration`](AccountPasskeyCommandService::open_session_for_passkey_registration),
//! [`get_passkey_registration_options`](AccountPasskeyCommandService::get_passkey_registration_options)
//! (hand `credentialCreationOptions.publicKey` to an authenticator), then
//! [`register_passkey`](AccountPasskeyCommandService::register_passkey) with
//! the authenticator's attestation.

use super::AccountPasskeyCommandService;
use crate::error::{Error, Result};
use crate::thrift::Value;

impl AccountPasskeyCommandService<'_> {
    /// Start a registration session. Returns its `sessionId`.
    pub async fn open_session_for_passkey_registration(&self) -> Result<String> {
        let res = self
            .call(
                "openSessionForPasskeyRegistration",
                crate::tv!({"request": {}}),
            )
            .await?;
        res.get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| Error::internal("PasskeyError", "no sessionId in response"))
    }

    /// WebAuthn creation options for the session
    /// (`{credentialCreationOptions: {mediation, publicKey}}`).
    pub async fn get_passkey_registration_options(&self, session_id: &str) -> Result<Value> {
        self.call(
            "getPasskeyRegistrationOptions",
            crate::tv!({"request": {"sessionId": session_id}}),
        )
        .await
    }

    /// Register the authenticator's response as a passkey named
    /// `passkey_name`. `credential` is a `RegistrationPublicKeyCredential`
    /// (`{id, type, response: {clientDataJson, attestationObject, transports},
    /// authenticatorAttachment, clientExtensionResults}`), base64url strings
    /// as WebAuthn returns them.
    pub async fn register_passkey(
        &self,
        session_id: &str,
        passkey_name: &str,
        credential: impl Into<Value>,
    ) -> Result<Value> {
        let mut req = crate::tv!({"sessionId": session_id, "passkeyName": passkey_name});
        req.insert("registrationPublicKeyCredential", credential.into());
        let mut args = Value::object();
        args.insert("request", req);
        self.call("registerPasskey", args).await
    }

    /// Rename a registered passkey.
    pub async fn update_passkey_name(
        &self,
        credential_id: &str,
        passkey_name: &str,
    ) -> Result<Value> {
        self.call(
            "updatePasskeyName",
            crate::tv!({"request": {"credentialId": credential_id, "passkeyName": passkey_name}}),
        )
        .await
    }

    /// Delete a registered passkey.
    pub async fn delete_passkey(&self, credential_id: &str) -> Result<Value> {
        self.call(
            "deletePasskey",
            crate::tv!({"request": {"credentialId": credential_id}}),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use crate::service::test_util::{encode, path, s};

    #[test]
    fn register_passkey_layout() {
        let args = encode(
            "registerPasskey_args",
            &crate::tv!({"request": {
                "sessionId": "sid",
                "passkeyName": "laptop",
                "registrationPublicKeyCredential": {
                    "id": "cred",
                    "type": "public-key",
                    "response": {"clientDataJson": "cd", "attestationObject": "ao", "transports": ["internal"]},
                },
            }}),
        );
        assert_eq!(path(&args, &[1, 1]), Some(&s("sid")));
        assert_eq!(path(&args, &[1, 2]), Some(&s("laptop")));
        assert_eq!(path(&args, &[1, 3, 3, 2]), Some(&s("ao")));
    }
}
