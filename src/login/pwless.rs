//! Passwordless login with a FIDO passkey ("PAAK", `SecondaryPwlessLogin`).
//!
//! The LINE desktop app (`SPSessionManagerImpl`) logs in without a password
//! by having the phone approve the session with a passkey. Its sequence on
//! `/acct/lgn/secpwless/v1` and `/acct/lp/lgn/secpwless/v1` is
//! `createSession` → `verifyLoginCertificate` → `requestPinCodeVerif` →
//! `checkPinCodeVerified` → `putExchangeKey` → `requestPaakAuth` →
//! `checkPaakAuthenticated` → `getE2eeKey` → `loginV2` (method and service
//! names derived from static RE of LINE macOS 26.5.0; unverified against
//! live server).
//!
//! Only `createSession` of that sequence has arg/result structs in the
//! bundled IDL, so the desktop-side flow cannot be completed here: the
//! request structs of the other eight methods (field ids and types) are not
//! known, and guessing them is not an option.
//!
//! What the IDL does define are the approving side's calls (from the
//! Android client, all raising `SecondaryPwlessLoginException`):
//! `getLoginActorContext`, `getChallengeForPaakAuth`, `authenticateWithPaak`,
//! `cancelPaakAuth`, `getExchangeKey` and `putE2eeKey`. They are exposed by
//! [`PaakApproval`]. The endpoint path serving them is not in the IDL, the
//! linejs sources or the macOS binary, so the caller passes it.

use crate::base::BaseClient;
use crate::error::{Error, Result};
use crate::service::{Service, rename_raw};
use crate::thrift::{Protocol, Schema, TValue, TypeRef, Value};
use crate::tv;

/// `SecondaryPwlessLoginService` path.
pub const SECONDARY_PWLESS_PATH: &str = "/acct/lgn/secpwless/v1";
/// Long-poll variant (`SecondaryPwlessLoginPermitNoticeService`; its
/// `checkPinCodeVerified` / `checkPaakAuthenticated` args are not in the
/// IDL).
pub const SECONDARY_PWLESS_LP_PATH: &str = "/acct/lp/lgn/secpwless/v1";

/// A WebAuthn assertion produced by the user's passkey for a challenge
/// from [`PaakApproval::get_challenge`] (IDL `AuthPublicKeyCredential`).
/// Binary fields are passed as the authenticator encodes them (the IDL
/// types them as strings; base64url per WebAuthn is the likely encoding).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaakCredential {
    pub id: String,
    /// Usually `public-key`.
    pub kind: String,
    pub client_data_json: String,
    pub authenticator_data: String,
    pub signature: String,
    pub user_handle: Option<String>,
    /// `extensionResults.lineAuthenSel`.
    pub line_authen_sel: Option<bool>,
}

impl PaakCredential {
    fn to_value(&self) -> Value {
        let mut response = tv!({
            "clientDataJSON": self.client_data_json,
            "authenticatorData": self.authenticator_data,
            "signature": self.signature,
        });
        if let (Value::Struct(m), Some(h)) = (&mut response, &self.user_handle) {
            m.insert("userHandle".into(), Value::from(h.as_str()));
        }
        let mut cred = tv!({"id": self.id, "type": self.kind});
        if let Value::Struct(m) = &mut cred {
            m.insert("response".into(), response);
            if let Some(sel) = self.line_authen_sel {
                m.insert("extensionResults".into(), tv!({"lineAuthenSel": sel}));
            }
        }
        cred
    }
}

fn create_session_args() -> Vec<(i16, TValue)> {
    vec![(1, TValue::Struct(Vec::new()))]
}

/// Approving side of a passwordless login (see the module docs).
#[derive(Clone, Copy)]
pub struct PaakApproval<'a> {
    service: Service<'a>,
}

impl BaseClient {
    /// `createSession` on [`SECONDARY_PWLESS_PATH`]: the first step of a
    /// passwordless login. Returns the session id.
    pub async fn create_pwless_session(&self) -> Result<String> {
        // `createSession_args.request` is the field-less `h80_C25643c`; send
        // it as an explicit empty struct (the schema encoder would drop it).
        let res = self
            .secondary_pwless_login()
            .request("createSession", create_session_args(), crate::request::Parse::Result)
            .await?;
        res["sessionId"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| Error::internal("LoginError", "createSession returned no sessionId"))
    }

    /// PAAK approval calls served at `path` (compact protocol).
    pub fn paak_approval(&self, path: &'static str) -> PaakApproval<'_> {
        PaakApproval { service: Service::new(self, path, Protocol::Compact) }
    }
}

impl PaakApproval<'_> {
    /// Who is trying to log in: `{applicationType, ipAddress, location}`.
    pub async fn get_login_actor_context(&self, session_id: &str) -> Result<Value> {
        self.service.call("getLoginActorContext", tv!({"request": {"sessionId": session_id}})).await
    }

    /// WebAuthn request options to sign with the passkey.
    ///
    /// The IDL types `options` as `o80_p80_j`, which it never defines, so
    /// the struct is decoded with the fields of the channel PAAK variant
    /// (`PublicKeyCredentialRequestOptions`: `challenge`, `timeout`, `rpId`,
    /// `allowCredentials`, `userVerification`, `extensions`). That shape is
    /// an assumption; unknown field ids are kept as numeric keys.
    pub async fn get_challenge(&self, session_id: &str) -> Result<Value> {
        let res = self.service.call("getChallengeForPaakAuth", tv!({"request": {"sessionId": session_id}})).await?;
        Ok(rename_raw(
            Schema::line(),
            &TypeRef::Named("PublicKeyCredentialRequestOptions".into()),
            res["options"].clone(),
        ))
    }

    /// Send the signed assertion.
    pub async fn authenticate(&self, session_id: &str, credential: &PaakCredential) -> Result<()> {
        let mut request = tv!({"sessionId": session_id});
        if let Value::Struct(m) = &mut request {
            m.insert("credential".into(), credential.to_value());
        }
        let mut args = tv!({});
        if let Value::Struct(m) = &mut args {
            m.insert("request".into(), request);
        }
        self.service.call("authenticateWithPaak", args).await.map(drop)
    }

    /// Abort the session.
    pub async fn cancel(&self, session_id: &str) -> Result<()> {
        self.service.call("cancelPaakAuth", tv!({"request": {"sessionId": session_id}})).await.map(drop)
    }

    /// The logging-in device's key-exchange data (`map<string,string>`).
    pub async fn get_exchange_key(&self, session_id: &str) -> Result<Value> {
        let res = self.service.call("getExchangeKey", tv!({"request": {"sessionId": session_id}})).await?;
        Ok(res["exchangeKey"].clone())
    }

    /// Hand the E2EE key material to the logging-in device.
    pub async fn put_e2ee_key(&self, session_id: &str, e2ee_key: &[(String, String)]) -> Result<()> {
        let map = Value::Map(e2ee_key.iter().map(|(k, v)| (Value::from(k.as_str()), Value::from(v.as_str()))).collect());
        let mut request = tv!({"sessionId": session_id});
        if let Value::Struct(m) = &mut request {
            m.insert("e2eeKey".into(), map);
        }
        let mut args = tv!({});
        if let Value::Struct(m) = &mut args {
            m.insert("request".into(), request);
        }
        self.service.call("putE2eeKey", args).await.map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thrift;

    fn encode(method: &str, args: Value) -> Vec<(i16, TValue)> {
        thrift::encode_struct(Schema::line(), &format!("{method}_args"), &args).unwrap()
    }

    #[test]
    fn create_session_args_are_an_empty_request() {
        assert_eq!(create_session_args(), vec![(1, TValue::Struct(vec![]))]);
        let def = Schema::line().get_struct("createSession_args").unwrap();
        assert_eq!(def.field_by_id(1).map(|f| f.name.as_str()), Some("request"));
    }

    #[test]
    fn authenticate_with_paak_encoding() {
        let cred = PaakCredential {
            id: "cid".into(),
            kind: "public-key".into(),
            client_data_json: "cd".into(),
            authenticator_data: "ad".into(),
            signature: "sig".into(),
            user_handle: Some("uh".into()),
            line_authen_sel: Some(true),
        };
        let mut request = tv!({"sessionId": "S"});
        if let Value::Struct(m) = &mut request {
            m.insert("credential".into(), cred.to_value());
        }
        let mut args = tv!({});
        if let Value::Struct(m) = &mut args {
            m.insert("request".into(), request);
        }
        let s = TValue::string;
        assert_eq!(
            encode("authenticateWithPaak", args),
            vec![(
                1,
                TValue::Struct(vec![
                    (1, s("S")),
                    (
                        2,
                        TValue::Struct(vec![
                            (1, s("cid")),
                            (2, s("public-key")),
                            (3, TValue::Struct(vec![(1, s("cd")), (2, s("ad")), (3, s("sig")), (4, s("uh"))])),
                            (4, TValue::Struct(vec![(91, TValue::Bool(true))])),
                        ])
                    ),
                ])
            )]
        );
    }

    #[test]
    fn session_id_requests_use_the_secondary_variant() {
        // The IDL defines two structs per name; the later (secondary pwless)
        // one with `sessionId` wins.
        for m in ["getChallengeForPaakAuth", "cancelPaakAuth", "getLoginActorContext", "getExchangeKey"] {
            assert_eq!(
                encode(m, tv!({"request": {"sessionId": "S"}})),
                vec![(1, TValue::Struct(vec![(1, TValue::string("S"))]))],
                "{m}"
            );
        }
    }
}
