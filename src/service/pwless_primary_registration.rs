//! `PwlessPrimaryRegistrationService`: pass-through methods generated from the linejs service.
//!
//! Path: `/ACCT/authfactor/pwless/v1`.

use super::PwlessPrimaryRegistrationService;
use crate::error::Result;
use crate::thrift::Value;

// <generated pass-through>
impl PwlessPrimaryRegistrationService<'_> {
    /// `createSession(request: h80_C25643c)`.
    pub async fn create_session(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("createSession", args).await
    }

    /// `getChallengeForPrimaryReg(request: GetChallengeForPrimaryRegRequest)`.
    pub async fn get_challenge_for_primary_reg(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getChallengeForPrimaryReg", args).await
    }
}
// </generated pass-through>
