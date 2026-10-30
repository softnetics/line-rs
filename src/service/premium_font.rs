//! `PremiumFontService`: pass-through methods generated from the linejs service.
//!
//! Path: `/EXT/talk/asset-meta-server/thrift/fontService`.

use super::PremiumFontService;
use crate::error::Result;
use crate::thrift::Value;

// <generated pass-through>
impl PremiumFontService<'_> {
    /// `getAccessToken(request: GetAccessTokenRequest)`.
    pub async fn get_access_token(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getAccessToken", args).await
    }

    /// `getFontMetas(request: GetFontMetasRequest)`.
    pub async fn get_font_metas(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getFontMetas", args).await
    }
}
// </generated pass-through>
