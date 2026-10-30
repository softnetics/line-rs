//! `PrimaryAccountSmartSwitchRestoreService`: pass-through methods generated from the linejs service.
//!
//! Path: `/ext/auth/feature-guest/thrift/account/v1/restore/smart-switch`.

use super::PrimaryAccountSmartSwitchRestoreService;
use crate::error::Result;
use crate::thrift::Value;

// <generated pass-through>
impl PrimaryAccountSmartSwitchRestoreService<'_> {
    /// `checkEmailAssigned(authSessionId: string, accountIdentifier: AccountIdentifier)`.
    pub async fn check_email_assigned(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("checkEmailAssigned", args).await
    }

    /// `checkIfPasswordSetVerificationEmailVerified(authSessionId: string)`.
    pub async fn check_if_password_set_verification_email_verified(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("checkIfPasswordSetVerificationEmailVerified", args).await
    }

    /// `getAcctVerifMethod(authSessionId: string, accountIdentifier: AccountIdentifier)`.
    pub async fn get_acct_verif_method(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getAcctVerifMethod", args).await
    }

    /// `getCountryInfo(authSessionId: string, simCard: SimCard)`.
    pub async fn get_country_info(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getCountryInfo", args).await
    }

    /// `getMaskedEmail(authSessionId: string, accountIdentifier: AccountIdentifier)`.
    pub async fn get_masked_email(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getMaskedEmail", args).await
    }

    /// `getPasswordHashingParametersForPwdReg(request: GetPasswordHashingParametersForPwdRegRequest)`.
    pub async fn get_password_hashing_parameters_for_pwd_reg(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getPasswordHashingParametersForPwdReg", args).await
    }

    /// `getPasswordHashingParametersForPwdVerif(request: GetPasswordHashingParametersForPwdVerifRequest)`.
    pub async fn get_password_hashing_parameters_for_pwd_verif(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getPasswordHashingParametersForPwdVerif", args).await
    }

    /// `getPhoneVerifMethodV2(request: GetPhoneVerifMethodV2Request)`.
    pub async fn get_phone_verif_method_v2(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getPhoneVerifMethodV2", args).await
    }

    /// `requestToSendPasswordSetVerificationEmail` (args struct `requestToSendPasswordSetVerificationEmail_args`).
    pub async fn request_to_send_password_set_verification_email(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("requestToSendPasswordSetVerificationEmail", args).await
    }

    /// `requestToSendPhonePinCode(request: ReqToSendPhonePinCodeRequest)`.
    pub async fn request_to_send_phone_pin_code(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("requestToSendPhonePinCode", args).await
    }

    /// `setHashedPassword(request: SetHashedPasswordRequest)`.
    pub async fn set_hashed_password(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("setHashedPassword", args).await
    }

    /// `verifyAccountUsingHashedPwd(request: VerifyAccountUsingHashedPwdRequest)`.
    pub async fn verify_account_using_hashed_pwd(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("verifyAccountUsingHashedPwd", args).await
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
