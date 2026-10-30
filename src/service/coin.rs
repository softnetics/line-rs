//! `CoinService`: pass-through methods generated from the linejs service.
//!
//! Path: `/COIN4`.

use super::CoinService;
use crate::error::Result;
use crate::thrift::Value;

// <generated pass-through>
impl CoinService<'_> {
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

    /// `getTotalCoinBalance(request: GetTotalCoinBalanceRequest)`.
    pub async fn get_total_coin_balance(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getTotalCoinBalance", args).await
    }

    /// `reserveCoinPurchase(request: CoinPurchaseReservation)`.
    pub async fn reserve_coin_purchase(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("reserveCoinPurchase", args).await
    }
}
// </generated pass-through>
