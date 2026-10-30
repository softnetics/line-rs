//! `DeviceAttestationService`: pass-through methods generated from the linejs service.
//!
//! Path: `/EXT/auth/feature-user/api/primary/device-attestation`.

use super::DeviceAttestationService;
use crate::error::Result;
use crate::thrift::Value;

// <generated pass-through>
impl DeviceAttestationService<'_> {
    /// `getAssertionChallenge(request: m80_l)`.
    pub async fn get_assertion_challenge(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getAssertionChallenge", args).await
    }

    /// `getAttestationChallenge(request: m80_n)`.
    pub async fn get_attestation_challenge(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getAttestationChallenge", args).await
    }

    /// `verifyAssertion(request: VerifyAssertionRequest)`.
    pub async fn verify_assertion(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("verifyAssertion", args).await
    }

    /// `verifyAttestation(request: VerifyAttestationRequest)`.
    pub async fn verify_attestation(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("verifyAttestation", args).await
    }
}
// </generated pass-through>
