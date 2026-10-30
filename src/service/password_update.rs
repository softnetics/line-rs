//! `PasswordUpdateService`: pass-through methods generated from the linejs service.
//!
//! Path: `/EXT/auth/feature-user/api/primary/password/update`.

use super::PasswordUpdateService;
use crate::error::Result;
use crate::thrift::Value;

// <generated pass-through>
impl PasswordUpdateService<'_> {
    /// `createSession(request: h80_C25643c)`.
    pub async fn create_session(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("createSession", args).await
    }

    /// `getPasswordHashingParameter(request: GetPasswordHashingParametersRequest)`.
    pub async fn get_password_hashing_parameter(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getPasswordHashingParameter", args).await
    }

    /// `setPassword(request: SetPasswordRequest)`.
    pub async fn set_password(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("setPassword", args).await
    }

    /// `updatePassword(request: UpdatePasswordRequest)`.
    pub async fn update_password(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("updatePassword", args).await
    }
}
// </generated pass-through>
