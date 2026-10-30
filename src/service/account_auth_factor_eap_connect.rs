//! `AccountAuthFactorEapConnectService`: pass-through methods generated from the linejs service.
//!
//! Path: `/ACCT/authfactor/eap/v1`.

use super::AccountAuthFactorEapConnectService;
use crate::error::Result;
use crate::thrift::Value;

// <generated pass-through>
impl AccountAuthFactorEapConnectService<'_> {
    /// `connectEapAccount(request: ConnectEapAccountRequest)`.
    pub async fn connect_eap_account(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("connectEapAccount", args).await
    }

    /// `disconnectEapAccount(request: DisconnectEapAccountRequest)`.
    pub async fn disconnect_eap_account(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("disconnectEapAccount", args).await
    }

    /// `openSession(request: OpenSessionRequest)`.
    pub async fn open_session(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("openSession", args).await
    }

    /// `verifyEapLogin(request: VerifyEapLoginRequest)`.
    pub async fn verify_eap_login(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("verifyEapLogin", args).await
    }
}
// </generated pass-through>
