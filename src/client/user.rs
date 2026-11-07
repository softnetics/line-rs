//! [`User`]: a contact (linejs `features/user`).

use super::{Client, list};
use crate::error::Result;
use crate::thrift::Value;
use crate::tv;

/// A user/contact. `raw` is a `GetContactV3Response`.
#[derive(Clone)]
pub struct User {
    client: Client,
    pub raw: Value,
}

impl std::fmt::Debug for User {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("User").field("raw", &self.raw).finish()
    }
}

impl User {
    pub fn new(client: Client, raw: Value) -> Self {
        Self { client, raw }
    }

    pub fn mid(&self) -> &str {
        self.raw["targetUserMid"].as_str().unwrap_or("")
    }

    /// Display name from the profile detail, if present.
    pub fn display_name(&self) -> Option<&str> {
        self.raw["targetProfileDetail"]["profileName"]
            .as_str()
            .or_else(|| self.raw["displayName"].as_str())
    }

    /// The profile picture path (`/0h…`), if any.
    pub fn picture_path(&self) -> Option<&str> {
        self.raw["targetProfileDetail"]["picturePath"]
            .as_str()
            .or_else(|| self.raw["picturePath"].as_str())
            .filter(|p| !p.is_empty())
    }

    /// The name the user gave this friend, if any.
    pub fn overridden_name(&self) -> Option<&str> {
        self.raw["friendDetail"]["user"]["overriddenName"]
            .as_str()
            .filter(|n| !n.is_empty())
    }

    /// An official account or bot (`userType` BOT).
    pub fn is_bot(&self) -> bool {
        user_type_is_bot(&self.raw["userType"])
    }

    /// Whether this is a friend (`friendDetail` is `user` or `bot`).
    pub fn is_friend(&self) -> bool {
        let d = &self.raw["friendDetail"];
        d.get("user").is_some() || d.get("bot").is_some()
    }

    /// Whether the account blocked this user (`blockDetail` is `user` or
    /// `bot`).
    pub fn is_blocked(&self) -> bool {
        let d = &self.raw["blockDetail"];
        d.get("user").is_some() || d.get("bot").is_some()
    }

    /// Whether notifications from this 1:1 chat are off, if the response
    /// says.
    pub fn muted(&self) -> Option<bool> {
        self.raw["notificationSettingEntry"]["notificationSetting"]["mute"].as_bool()
    }

    /// Turn notifications for the 1:1 chat off (`true`) or on, with
    /// `updateNotificationSetting`.
    pub async fn set_muted(&self, muted: bool) -> Result<()> {
        self.client
            .base
            .talk()
            .update_notification_setting(self.mid(), muted)
            .await?;
        Ok(())
    }

    /// Block this user (`blockRelation`).
    pub async fn block(&self) -> Result<()> {
        self.client
            .base
            .relation()
            .block_relation(self.mid())
            .await?;
        Ok(())
    }

    /// Unblock this user (`unblockRelation`).
    pub async fn unblock(&self) -> Result<()> {
        self.client
            .base
            .relation()
            .unblock_relation(self.mid(), None)
            .await?;
        Ok(())
    }

    /// Contact calendar events (e.g. birthday). `event_kinds` are
    /// `Pb1_EnumC13096n3` names/values; `None` asks for all.
    pub async fn fetch_calendar_events(
        &self,
        event_kinds: Option<Vec<Value>>,
    ) -> Result<Vec<Value>> {
        let mut request = tv!({"targetUsers": [{"targetUserMid": self.mid()}]});
        if let Some(kinds) = event_kinds {
            request.insert("requiredContactCalendarEvents", Value::List(kinds));
        }
        let mut args = Value::object();
        args.insert("request", request);
        let res = self
            .client
            .base
            .relation()
            .call("getContactCalendarEvents", args)
            .await?;
        let entry = list(&res["responses"])
            .into_iter()
            .find(|r| r["targetUserMid"].as_str() == Some(self.mid()));
        Ok(entry
            .map(|e| {
                e["ContactCalendarEvents"]["events"]
                    .entries()
                    .into_iter()
                    .map(|(_, v)| v.clone())
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Send a compact talk message through `/CA5` or `/ECA5`.
    pub async fn send_compact_message(
        &self,
        text: &str,
    ) -> Result<crate::service::talk::compact::CompactMessageResponse> {
        self.client.send_compact_message(self.mid(), text).await
    }

    async fn update_setting(&self, flag: &str, value: &str) -> Result<()> {
        let seq = self.client.base.get_reqseq("talk").await?;
        self.client
            .base
            .talk()
            .call(
                "updateContactSetting",
                tv!({"reqSeq": seq, "mid": self.mid(), "flag": flag, "value": value}),
            )
            .await?;
        Ok(())
    }

    /// Set (`Some`) or clear (`None`) the display-name override.
    pub async fn rename(&self, display_name_override: Option<&str>) -> Result<()> {
        self.update_setting(
            "CONTACT_SETTING_DISPLAY_NAME_OVERRIDE",
            display_name_override.unwrap_or(""),
        )
        .await
    }

    pub async fn set_favorite(&self, favorite: bool) -> Result<()> {
        self.update_setting("CONTACT_SETTING_FAVORITE", bool_str(favorite))
            .await
    }

    /// The legacy `updateContactSetting` mute, no longer used by LINE's
    /// clients; see [`set_muted`](Self::set_muted).
    pub async fn set_notification_disabled(&self, disabled: bool) -> Result<()> {
        self.update_setting("CONTACT_SETTING_NOTIFICATION_DISABLE", bool_str(disabled))
            .await
    }

    /// Hide this contact from the friend list.
    pub async fn set_hidden(&self, hidden: bool) -> Result<()> {
        self.update_setting("CONTACT_SETTING_CONTACT_HIDE", bool_str(hidden))
            .await
    }
}

/// `userType` (`LN0_X0`) as the enum name or number: BOT is 2.
fn user_type_is_bot(v: &Value) -> bool {
    v.as_str() == Some("BOT") || v.as_i64() == Some(2)
}

fn bool_str(b: bool) -> &'static str {
    if b { "true" } else { "false" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::BaseClient;
    use crate::device::Device;

    #[test]
    fn reads_a_get_contact_v3_response() {
        let client = Client::new(BaseClient::new(Device::Android).unwrap());
        let friend = User::new(
            client.clone(),
            tv!({
                "targetUserMid": "u1",
                "userType": "USER",
                "targetProfileDetail": {"profileName": "A", "picturePath": "/0hx"},
                "friendDetail": {"user": {"createdTime": 1, "overriddenName": "Ann"}},
                "blockDetail": {"notBlocked": {}},
                "notificationSettingEntry": {"notificationSetting": {"mute": true}},
            }),
        );
        assert_eq!(friend.display_name(), Some("A"));
        assert_eq!(friend.overridden_name(), Some("Ann"));
        assert_eq!(friend.picture_path(), Some("/0hx"));
        assert!(friend.is_friend() && !friend.is_blocked() && !friend.is_bot());
        assert_eq!(friend.muted(), Some(true));
        let blocked_bot = User::new(
            client,
            tv!({
                "targetUserMid": "u2",
                "userType": 2,
                "friendDetail": {"notFriend": {}},
                "blockDetail": {"bot": {"deletedFromBlockList": false}},
            }),
        );
        assert!(blocked_bot.is_bot() && blocked_bot.is_blocked() && !blocked_bot.is_friend());
    }
}
