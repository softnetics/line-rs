//! `PrimaryAccountReLoginService`: pass-through methods generated from the linejs service.
//!
//! Path: `/ext/auth/feature-guest/thrift/account/v1/relogin/primary`.

use super::PrimaryAccountReLoginService;
use crate::error::Result;
use crate::thrift::Value;

// <generated pass-through>
impl PrimaryAccountReLoginService<'_> {
    /// `createSession(request: h80_C25643c)`.
    pub async fn create_session(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("createSession", args).await
    }

    /// `getCountryInfo(authSessionId: string, simCard: SimCard)`.
    pub async fn get_country_info(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getCountryInfo", args).await
    }

    /// `getPhoneVerifMethodV2(request: GetPhoneVerifMethodV2Request)`.
    pub async fn get_phone_verif_method_v2(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getPhoneVerifMethodV2", args).await
    }

    /// `requestToSendPhonePinCode(request: ReqToSendPhonePinCodeRequest)`.
    pub async fn request_to_send_phone_pin_code(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("requestToSendPhonePinCode", args).await
    }

    /// `verifyEapLogin(request: VerifyEapLoginRequest)`.
    pub async fn verify_eap_login(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("verifyEapLogin", args).await
    }

    /// `verifyPhonePinCode(request: VerifyPhonePinCodeRequest)`.
    pub async fn verify_phone_pin_code(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("verifyPhonePinCode", args).await
    }
}
// </generated pass-through>
