//! The account's notification settings (`Settings`, from `getSettings`),
//! as LINE for Android reads them to decide on a banner (`ts8/ah.java`).

use super::Client;
use crate::error::Result;
use crate::thrift::Value;

/// The notification part of the account's `Settings`. A field is `None`
/// when the server left it out; LINE's defaults are on for every switch
/// here, so treat `None` as `true` (see [`NotificationSettings::on`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NotificationSettings {
    /// `notificationEnable` (10): notifications on at all.
    pub enabled: Option<bool>,
    /// `notificationMuteExpiration` (11): notifications are paused until
    /// this time, epoch milliseconds (unit **unverified**). 0 or a past
    /// time means not paused.
    pub mute_expiration: i64,
    /// `notificationNewMessage` (12).
    pub new_message: Option<bool>,
    /// `notificationGroupInvitation` (13).
    pub group_invitation: Option<bool>,
    /// `notificationShowMessage` (14): show the message text in the banner.
    pub show_message: Option<bool>,
    /// `notificationIncomingCall` (15).
    pub incoming_call: Option<bool>,
    /// `notificationDisabledWithSub` (18): what it silences is
    /// **unverified** (by its name, the primary device while a
    /// secondary one is in use).
    pub disabled_with_sub: Option<bool>,
    /// `notificationMention` (68): mentions notify in muted chats.
    pub mention: Option<bool>,
    /// `notificationThumbnail` (72): show image thumbnails.
    pub thumbnail: Option<bool>,
    /// `notificationBadgeTalkOnly` (93): the app badge counts chats only.
    pub badge_talk_only: Option<bool>,
    /// `notificationReaction` (95): reactions to your messages notify.
    pub reaction: Option<bool>,
    /// `notificationShowProfileImage` (104).
    pub show_profile_image: Option<bool>,
}

/// `SettingsAttributeEx` values of the notification fields, as an
/// `UPDATE_SETTINGS` operation lists them (`ts8/bh.java`). Attribute ids
/// are not field ids.
pub const NOTIFICATION_ATTRIBUTES: &[i32] = &[0, 1, 2, 3, 4, 5, 16, 40, 45, 65, 67, 75];

impl NotificationSettings {
    /// Read a decoded `Settings` struct.
    pub fn from_settings(v: &Value) -> Self {
        let b = |k: &str| v[k].as_bool();
        Self {
            enabled: b("notificationEnable"),
            mute_expiration: v["notificationMuteExpiration"].as_i64().unwrap_or(0),
            new_message: b("notificationNewMessage"),
            group_invitation: b("notificationGroupInvitation"),
            show_message: b("notificationShowMessage"),
            incoming_call: b("notificationIncomingCall"),
            disabled_with_sub: b("notificationDisabledWithSub"),
            mention: b("notificationMention"),
            thumbnail: b("notificationThumbnail"),
            badge_talk_only: b("notificationBadgeTalkOnly"),
            reaction: b("notificationReaction"),
            show_profile_image: b("notificationShowProfileImage"),
        }
    }

    /// A switch's value, on when the server didn't send it.
    pub fn on(field: Option<bool>) -> bool {
        field.unwrap_or(true)
    }

    /// Whether notifications are paused at `now_ms` by
    /// `notificationMuteExpiration`.
    pub fn paused_at(&self, now_ms: i64) -> bool {
        self.mute_expiration > now_ms
    }
}

/// The `SettingsAttributeEx` ids an `UPDATE_SETTINGS` (36) operation names
/// in `param3`: decimal strings joined by `\x1e` (`uq8/h5.java`).
pub fn updated_setting_attributes(param3: &str) -> Vec<i32> {
    param3
        .split('\u{1e}')
        .filter_map(|s| s.trim().parse().ok())
        .collect()
}

impl Client {
    /// The account's notification settings, from `getSettings`.
    pub async fn notification_settings(&self) -> Result<NotificationSettings> {
        let settings = self.base.talk().get_settings(crate::tv!({})).await?;
        Ok(NotificationSettings::from_settings(&settings))
    }

    /// The account's `preferenceLocale` (e.g. `th_TH`), if set.
    pub async fn preference_locale(&self) -> Result<Option<String>> {
        let settings = self.base.talk().get_settings(crate::tv!({})).await?;
        Ok(settings["preferenceLocale"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_string))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tv;

    #[test]
    fn reads_the_notification_fields() {
        let s = NotificationSettings::from_settings(&tv!({
            "notificationEnable": true,
            "notificationMuteExpiration": 1_800_000_000_000_i64,
            "notificationShowMessage": false,
            "notificationMention": true,
            "notificationReaction": false,
            "notificationBadgeTalkOnly": true,
        }));
        assert_eq!(s.enabled, Some(true));
        assert_eq!(s.show_message, Some(false));
        assert_eq!(s.reaction, Some(false));
        assert_eq!(s.badge_talk_only, Some(true));
        assert_eq!(s.new_message, None);
        assert!(NotificationSettings::on(s.new_message));
        assert!(s.paused_at(1_700_000_000_000));
        assert!(!s.paused_at(1_900_000_000_000));
        assert!(!NotificationSettings::default().paused_at(0));
    }

    #[test]
    fn reads_update_settings_attributes() {
        assert_eq!(updated_setting_attributes("0\u{1e}4\u{1e}67"), [0, 4, 67]);
        assert_eq!(updated_setting_attributes(""), Vec::<i32>::new());
        assert!(
            updated_setting_attributes("40")
                .iter()
                .any(|a| NOTIFICATION_ATTRIBUTES.contains(a))
        );
    }
}
