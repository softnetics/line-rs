//! `PremiumProductDefinitionService`: LYP/LINE Premium product tiers.
//!
//! Path: `/EXT/line-premium/common/thrift/product-definition`.
//!
//! Recovered by static RE of LINE for macOS 26.5.0 (unverified against a live
//! server). The request (`GetProductTiersRequest { 1: string region }`) is
//! taken from the generated `*_pargs::write` code; the response
//! (`GetProductTiersResponse`, a `map<string, ProductTier>`) is returned as a
//! raw [`Value`] because the `ProductTier` layout was not fully recovered.

use super::PremiumProductDefinitionService;
use crate::error::Result;
use crate::request::Parse;
use crate::thrift::Value;

impl PremiumProductDefinitionService<'_> {
    /// `getProductTiers(req: GetProductTiersRequest)`. Pass e.g.
    /// `tv!({"req": {"region": "JP"}})`. Returns the raw success value.
    pub async fn get_product_tiers(&self, args: impl Into<Value>) -> Result<Value> {
        self.call_with("getProductTiers", args, |o| o.parse(Parse::Raw)).await
    }

    /// `getProductTiersForClient(req: GetProductTiersRequest)`. Returns the raw
    /// success value.
    pub async fn get_product_tiers_for_client(&self, args: impl Into<Value>) -> Result<Value> {
        self.call_with("getProductTiersForClient", args, |o| o.parse(Parse::Raw)).await
    }
}
