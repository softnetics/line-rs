//! `ChannelService`: pass-through methods generated from the linejs service.
//!
//! Path: `/CH4`.

use super::ChannelService;
use crate::error::Result;
use crate::request::Parse;
use crate::thrift::{TValue, Value};

impl ChannelService<'_> {
    /// Revoke an approved channel by id.
    ///
    /// Recovered by static RE of LINE macOS 26.5.0; unverified against a live
    /// server.
    pub async fn revoke_channel(&self, channel_id: &str) -> Result<Value> {
        self.request("revokeChannel", vec![(1, TValue::string(channel_id))], Parse::Result).await
    }

    /// The caller's channel settings (`ChannelSettings`). Recovered by static
    /// RE of LINE macOS 26.5.0; unverified against a live server.
    pub async fn get_channel_settings(&self) -> Result<Value> {
        self.request("getChannelSettings", Vec::new(), Parse::Result).await
    }
}

// <generated pass-through>
impl ChannelService<'_> {
    /// `approveChannelAndIssueChannelToken(channelId: string)`.
    pub async fn approve_channel_and_issue_channel_token(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("approveChannelAndIssueChannelToken", args).await
    }

    /// `getApprovedChannels(lastSynced: i64, locale: string)`.
    pub async fn get_approved_channels(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getApprovedChannels", args).await
    }

    /// `getChannelInfo(channelId: string, locale: string)`.
    pub async fn get_channel_info(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getChannelInfo", args).await
    }

    /// `getChannelNotificationSettings(locale: string)`.
    pub async fn get_channel_notification_settings(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getChannelNotificationSettings", args).await
    }

    /// `getCommonDomains(lastSynced: i64)`.
    pub async fn get_common_domains(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getCommonDomains", args).await
    }

    /// `getReturnUrlWithRequestTokenForAutoLogin(webLoginRequest: WebLoginRequest)`.
    pub async fn get_return_url_with_request_token_for_auto_login(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getReturnUrlWithRequestTokenForAutoLogin", args).await
    }

    /// `getUpdatedChannelIds(channelIds: list<ChannelIdWithLastUpdated>)`.
    pub async fn get_updated_channel_ids(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getUpdatedChannelIds", args).await
    }

    /// `getWebLoginDisallowedUrl(webLoginRequest: WebLoginRequest)`.
    pub async fn get_web_login_disallowed_url(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getWebLoginDisallowedUrl", args).await
    }

    /// `issueChannelToken(channelId: string)`.
    pub async fn issue_channel_token(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("issueChannelToken", args).await
    }

    /// `issueRequestTokenWithAuthScheme` (args struct `issueRequestTokenWithAuthScheme_args`).
    pub async fn issue_request_token_with_auth_scheme(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("issueRequestTokenWithAuthScheme", args).await
    }

    /// `updateChannelNotificationSetting(setting: list<ChannelNotificationSetting>)`.
    pub async fn update_channel_notification_setting(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("updateChannelNotificationSetting", args).await
    }

    /// `updateChannelSettings(channelSettings: ChannelSettings)`.
    pub async fn update_channel_settings(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("updateChannelSettings", args).await
    }
}
// </generated pass-through>
