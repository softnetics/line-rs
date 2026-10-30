//! [`User`]: a contact (linejs `features/user`).

use super::{list, Client};
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
        self.raw["targetProfileDetail"]["profileName"].as_str().or_else(|| self.raw["displayName"].as_str())
    }

    /// Contact calendar events (e.g. birthday). `event_kinds` are
    /// `Pb1_EnumC13096n3` names/values; `None` asks for all.
    pub async fn fetch_calendar_events(&self, event_kinds: Option<Vec<Value>>) -> Result<Vec<Value>> {
        let mut request = tv!({"targetUsers": [{"targetUserMid": self.mid()}]});
        if let Some(kinds) = event_kinds {
            request.insert("requiredContactCalendarEvents", Value::List(kinds));
        }
        let mut args = Value::object();
        args.insert("request", request);
        let res = self.client.base.relation().call("getContactCalendarEvents", args).await?;
        let entry = list(&res["responses"]).into_iter().find(|r| r["targetUserMid"].as_str() == Some(self.mid()));
        Ok(entry
            .map(|e| e["ContactCalendarEvents"]["events"].entries().into_iter().map(|(_, v)| v.clone()).collect())
            .unwrap_or_default())
    }

    /// Send a compact talk message through `/CA5` or `/ECA5`.
    pub async fn send_compact_message(&self, text: &str) -> Result<crate::service::talk::compact::CompactMessageResponse> {
        self.client.send_compact_message(self.mid(), text).await
    }

    async fn update_setting(&self, flag: &str, value: &str) -> Result<()> {
        let seq = self.client.base.get_reqseq("talk").await?;
        self.client
            .base
            .talk()
            .call("updateContactSetting", tv!({"reqSeq": seq, "mid": self.mid(), "flag": flag, "value": value}))
            .await?;
        Ok(())
    }

    /// Set (`Some`) or clear (`None`) the display-name override.
    pub async fn rename(&self, display_name_override: Option<&str>) -> Result<()> {
        self.update_setting("CONTACT_SETTING_DISPLAY_NAME_OVERRIDE", display_name_override.unwrap_or("")).await
    }

    pub async fn set_favorite(&self, favorite: bool) -> Result<()> {
        self.update_setting("CONTACT_SETTING_FAVORITE", bool_str(favorite)).await
    }

    pub async fn set_notification_disabled(&self, disabled: bool) -> Result<()> {
        self.update_setting("CONTACT_SETTING_NOTIFICATION_DISABLE", bool_str(disabled)).await
    }

    /// Hide this contact from the friend list.
    pub async fn set_hidden(&self, hidden: bool) -> Result<()> {
        self.update_setting("CONTACT_SETTING_CONTACT_HIDE", bool_str(hidden)).await
    }
}

fn bool_str(b: bool) -> &'static str {
    if b { "true" } else { "false" }
}
