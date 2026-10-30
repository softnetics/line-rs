//! `PrimaryAccountInitService`: pass-through methods generated from the linejs service.
//!
//! Path: `/acct/pais/v1`.

use super::PrimaryAccountInitService;
use crate::error::Result;
use crate::thrift::Value;

// <generated pass-through>
impl PrimaryAccountInitService<'_> {
    /// `getAcctVerifMethod(authSessionId: string, accountIdentifier: AccountIdentifier)`.
    pub async fn get_acct_verif_method(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getAcctVerifMethod", args).await
    }

    /// `getAllowedRegistrationMethod(authSessionId: string, countryCode: string)`.
    pub async fn get_allowed_registration_method(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getAllowedRegistrationMethod", args).await
    }

    /// `getCountryInfo(authSessionId: string, simCard: SimCard)`.
    pub async fn get_country_info(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getCountryInfo", args).await
    }

    /// `getPasswordHashingParametersForPwdReg(request: GetPasswordHashingParametersForPwdRegRequest)`.
    pub async fn get_password_hashing_parameters_for_pwd_reg(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getPasswordHashingParametersForPwdReg", args).await
    }

    /// `getPhoneVerifMethodForRegistration(request: GetPhoneVerifMethodForRegistrationRequest)`.
    pub async fn get_phone_verif_method_for_registration(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getPhoneVerifMethodForRegistration", args).await
    }

    /// `getPhoneVerifMethodV2(request: GetPhoneVerifMethodV2Request)`.
    pub async fn get_phone_verif_method_v2(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getPhoneVerifMethodV2", args).await
    }

    /// `lookupAvailableEap(request: LookupAvailableEapRequest)`.
    pub async fn lookup_available_eap(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("lookupAvailableEap", args).await
    }

    /// `openSession(request: OpenSessionRequest)`.
    pub async fn open_session(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("openSession", args).await
    }

    /// `registerPrimaryUsingEapAccount(authSessionId: string)`.
    pub async fn register_primary_using_eap_account(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("registerPrimaryUsingEapAccount", args).await
    }

    /// `registerPrimaryUsingPhoneWithTokenV3(authSessionId: string)`.
    pub async fn register_primary_using_phone_with_token_v3(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("registerPrimaryUsingPhoneWithTokenV3", args).await
    }

    /// `requestToSendPhonePinCode(request: ReqToSendPhonePinCodeRequest)`.
    pub async fn request_to_send_phone_pin_code(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("requestToSendPhonePinCode", args).await
    }

    /// `setHashedPassword(request: SetHashedPasswordRequest)`.
    pub async fn set_hashed_password(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("setHashedPassword", args).await
    }

    /// `setPassword(request: SetPasswordRequest)`.
    pub async fn set_password(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("setPassword", args).await
    }

    /// `validateProfile(authSessionId: string, displayName: string)`.
    pub async fn validate_profile(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("validateProfile", args).await
    }

    /// `verifyEapAccountForRegistration(authSessionId: string, device: Device, socialLogin: SocialLogin)`.
    pub async fn verify_eap_account_for_registration(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("verifyEapAccountForRegistration", args).await
    }

    /// `verifyPhonePinCode(request: VerifyPhonePinCodeRequest)`.
    pub async fn verify_phone_pin_code(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("verifyPhonePinCode", args).await
    }
}
// </generated pass-through>
