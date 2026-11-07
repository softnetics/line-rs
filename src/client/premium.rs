//! LINE Premium (LYP Premium) status of the logged-in account.

use super::Client;
use crate::error::Result;
use crate::thrift::Value;

/// Whether the account has LINE Premium, from
/// `PremiumStatusService.getPremiumStatus`
/// (`/EXT/line-premium/common/thrift/status`). Features such as editing a message
/// or unsending silently answer `NOT_PREMIUM` without it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PremiumStatus {
    /// Premium is on now.
    pub active: bool,
    /// `INACTIVE`, `ACTIVE_FINITE` or `ACTIVE_INFINITE` when the server sends it.
    pub status: Option<String>,
    /// End of the paid period, epoch milliseconds (0 when unknown).
    pub valid_until: i64,
    /// Next payment, epoch milliseconds (0 when unknown).
    pub next_payment_time: i64,
    pub on_free_trial: bool,
    /// The subscription is cancelled and ends at `valid_until`.
    pub will_expire: bool,
    /// How it was bought, e.g. `LINE_APPLE` or `SOFTBANK_BUNDLE`.
    pub plan_type: Option<String>,
}

impl PremiumStatus {
    /// Read a decoded `GetPremiumStatusResponse`.
    pub fn from_value(v: &Value) -> Self {
        let name = |v: &Value| {
            v.as_str()
                .map(str::to_string)
                .or_else(|| v.as_i64().map(|n| n.to_string()))
        };
        let status = name(&v["status"]);
        // `active` is the plain flag; an ACTIVE_* status says the same.
        let active = v["active"].as_bool().unwrap_or(false)
            || status
                .as_deref()
                .is_some_and(|s| s.starts_with("ACTIVE") || s == "2" || s == "3");
        Self {
            active,
            status,
            valid_until: v["validUntil"].as_i64().unwrap_or(0),
            next_payment_time: v["nextPaymentTime"].as_i64().unwrap_or(0),
            on_free_trial: v["onFreeTrial"].as_bool().unwrap_or(false),
            will_expire: v["willExpire"].as_bool().unwrap_or(false),
            plan_type: name(&v["planType"]),
        }
    }
}

/// `LinePremiumStatusService`.
const LINE_PREMIUM_STATUS_PATH: &str = "/EXT/line-premium/common/thrift/line-premium-status";

/// Which status service answers for an account in `region`: the
/// LYP Premium one (`PremiumStatusService.getPremiumStatus`) for Japan,
/// the LINE Premium one (`LinePremiumStatusService.getLinePremiumStatus`)
/// elsewhere, picked by the profile's region.
pub(crate) fn uses_lyp_premium_service(region: Option<&str>) -> bool {
    region.is_some_and(|r| r.eq_ignore_ascii_case("JP"))
}

impl Client {
    /// The account's LINE Premium status. Read-only.
    ///
    /// Asks the one service for the profile's region:
    /// `PremiumStatusService.getPremiumStatus`
    /// (`/EXT/line-premium/common/thrift/status`) for a Japanese account,
    /// `LinePremiumStatusService.getLinePremiumStatus`
    /// (`/EXT/line-premium/common/thrift/line-premium-status`)
    /// for any other. The other service is not tried.
    pub async fn premium_status(&self) -> Result<PremiumStatus> {
        let res = if uses_lyp_premium_service(self.base.region_code().as_deref()) {
            self.base
                .premium_status()
                .get_premium_status(crate::tv!({"req": {}}))
                .await?
        } else {
            self.base
                .call(
                    "getLinePremiumStatus",
                    crate::tv!({"req": {}}),
                    LINE_PREMIUM_STATUS_PATH,
                    crate::thrift::Protocol::Compact,
                )
                .await?
        };
        Ok(PremiumStatus::from_value(&res))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_an_active_subscription() {
        let v = crate::tv!({
            "active": true,
            "status": "ACTIVE_FINITE",
            "validUntil": 1_800_000_000_000_i64,
            "planType": "LINE_GOOGLE",
            "onFreeTrial": false,
            "willExpire": true
        });
        let s = PremiumStatus::from_value(&v);
        assert!(s.active);
        assert!(s.will_expire);
        assert_eq!(s.valid_until, 1_800_000_000_000);
        assert_eq!(s.plan_type.as_deref(), Some("LINE_GOOGLE"));
    }

    #[test]
    fn missing_fields_mean_not_premium() {
        let s = PremiumStatus::from_value(&crate::tv!({}));
        assert_eq!(s, PremiumStatus::default());
        assert!(!s.active);
    }

    #[test]
    fn service_is_picked_by_region() {
        assert!(uses_lyp_premium_service(Some("JP")));
        assert!(uses_lyp_premium_service(Some("jp")));
        assert!(!uses_lyp_premium_service(Some("TH")));
        assert!(!uses_lyp_premium_service(None));
    }

    #[test]
    fn status_alone_counts() {
        let s = PremiumStatus::from_value(&crate::tv!({"status": 3}));
        assert!(s.active);
        let s = PremiumStatus::from_value(&crate::tv!({"status": "INACTIVE"}));
        assert!(!s.active);
    }
}
