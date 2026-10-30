//! `ShopService`: pass-through methods generated from the linejs service.
//!
//! Path: `/TSHOP4`.

use super::ShopService;
use crate::error::Result;
use crate::thrift::Value;

// <generated pass-through>
impl ShopService<'_> {
    /// `addThemeToSubscriptionSlot(req: AddThemeToSubscriptionSlotRequest)`.
    pub async fn add_theme_to_subscription_slot(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("addThemeToSubscriptionSlot", args).await
    }

    /// `canReceivePresent(shopId: string, productId: string, locale: Locale, recipientMid: string)`.
    pub async fn can_receive_present(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("canReceivePresent", args).await
    }

    /// `createCombinationSticker(request: YN0_Ob1_C)`.
    pub async fn create_combination_sticker(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("createCombinationSticker", args).await
    }

    /// `getAutoSuggestionShowcase(autoSuggestionShowcaseRequest: AutoSuggestionShowcaseRequest)`.
    pub async fn get_auto_suggestion_showcase(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getAutoSuggestionShowcase", args).await
    }

    /// `getCoinProducts(request: GetCoinProductsRequest)`.
    pub async fn get_coin_products(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getCoinProducts", args).await
    }

    /// `getCoinPurchaseHistory(request: GetCoinHistoryRequest)`.
    pub async fn get_coin_purchase_history(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getCoinPurchaseHistory", args).await
    }

    /// `getCoinUseAndRefundHistory(request: GetCoinHistoryRequest)`.
    pub async fn get_coin_use_and_refund_history(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getCoinUseAndRefundHistory", args).await
    }

    /// `getMusicSubscriptionStatus(request: YN0_Ob1_N)`.
    pub async fn get_music_subscription_status(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getMusicSubscriptionStatus", args).await
    }

    /// `getOwnedProductSummaries(shopId: string, offset: i32, limit: i32, locale: Locale)`.
    pub async fn get_owned_product_summaries(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getOwnedProductSummaries", args).await
    }

    /// `getProductByVersion(shopId: string, productId: string, productVersion: i64, locale: Locale)`.
    pub async fn get_product_by_version(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getProductByVersion", args).await
    }

    /// `getProductLatestVersionForUser(request: YN0_Ob1_P)`.
    pub async fn get_product_latest_version_for_user(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getProductLatestVersionForUser", args).await
    }

    /// `getProductV2(request: YN0_Ob1_S)`.
    pub async fn get_product_v2(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getProductV2", args).await
    }

    /// `getProductValidationScheme(shopId: string, productId: string, productVersion: i64)`.
    pub async fn get_product_validation_scheme(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getProductValidationScheme", args).await
    }

    /// `getProductsByAuthor(productListByAuthorRequest: YN0_Ob1_G0)`.
    pub async fn get_products_by_author(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getProductsByAuthor", args).await
    }

    /// `getPurchasedProducts(shopId: string, offset: i32, limit: i32, locale: Locale)`.
    pub async fn get_purchased_products(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getPurchasedProducts", args).await
    }

    /// `getReceivedPresents(shopId: string, offset: i32, limit: i32, locale: Locale)`.
    pub async fn get_received_presents(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getReceivedPresents", args).await
    }

    /// `getRecommendationList(getRecommendationRequest: YN0_Ob1_W)`.
    pub async fn get_recommendation_list(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getRecommendationList", args).await
    }

    /// `getResourceFile(req: YN0_Ob1_Z)`.
    pub async fn get_resource_file(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getResourceFile", args).await
    }

    /// `getSentPresents(shopId: string, offset: i32, limit: i32, locale: Locale)`.
    pub async fn get_sent_presents(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getSentPresents", args).await
    }

    /// `getStudentInformation(req: Ob1_C12606a0)`.
    pub async fn get_student_information(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getStudentInformation", args).await
    }

    /// `getSubscriptionStatus(req: GetSubscriptionStatusRequest)`.
    pub async fn get_subscription_status(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getSubscriptionStatus", args).await
    }

    /// `getSuggestResourcesV2(req: GetSuggestResourcesV2Request)`.
    pub async fn get_suggest_resources_v2(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getSuggestResourcesV2", args).await
    }

    /// `notifyProductEvent(shopId: string, productId: string, productVersion: i64, productEvent: i64)`.
    pub async fn notify_product_event(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("notifyProductEvent", args).await
    }

    /// `placePurchaseOrderForFreeProduct(purchaseOrder: PurchaseOrder)`.
    pub async fn place_purchase_order_for_free_product(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("placePurchaseOrderForFreeProduct", args).await
    }

    /// `placePurchaseOrderWithLineCoin(purchaseOrder: PurchaseOrder)`.
    pub async fn place_purchase_order_with_line_coin(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("placePurchaseOrderWithLineCoin", args).await
    }

    /// `reserveCoinPurchase(request: CoinPurchaseReservation)`.
    pub async fn reserve_coin_purchase(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("reserveCoinPurchase", args).await
    }

    /// `shouldShowWelcomeStickerBanner(request: Ob1_C12660s1)`.
    pub async fn should_show_welcome_sticker_banner(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("shouldShowWelcomeStickerBanner", args).await
    }

    /// `stopBundleSubscription(request: StopBundleSubscriptionRequest)`.
    pub async fn stop_bundle_subscription(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("stopBundleSubscription", args).await
    }

    /// `validateProduct(shopId: string, productId: string, productVersion: i64, validationReq: YN0_Ob1_Q0)`.
    pub async fn validate_product(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("validateProduct", args).await
    }
}
// </generated pass-through>
