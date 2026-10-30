//! Own-profile helpers (linejs `features/profile.ts`).

use super::Client;
use crate::error::{Error, Result};
use crate::thrift::{Map, Value};
use crate::tv;

/// `ProfileAttribute` bit values (IDL enum `Pb1_K6`), used as the keys of
/// `updateProfileAttributes`.
pub struct ProfileAttribute;

impl ProfileAttribute {
    pub const EMAIL: i32 = 1;
    pub const DISPLAY_NAME: i32 = 2;
    pub const PHONETIC_NAME: i32 = 4;
    pub const PICTURE: i32 = 8;
    pub const STATUS_MESSAGE: i32 = 16;
    pub const ALLOW_SEARCH_BY_USERID: i32 = 32;
    pub const ALLOW_SEARCH_BY_EMAIL: i32 = 64;
    pub const BUDDY_STATUS: i32 = 128;
    pub const MUSIC_PROFILE: i32 = 256;
    pub const AVATAR_PROFILE: i32 = 512;
    pub const HIDDEN_FROM_LIST: i32 = 1024;
}

/// Sparse profile update: only `Some` fields are sent.
#[derive(Debug, Clone, Default)]
pub struct MyProfileUpdate {
    pub display_name: Option<String>,
    pub status_message: Option<String>,
    pub phonetic_name: Option<String>,
    pub music_profile: Option<String>,
    pub allow_search_by_userid: Option<bool>,
    pub allow_search_by_email: Option<bool>,
    pub hidden_from_list: Option<bool>,
}

impl MyProfileUpdate {
    /// `(attribute, value)` pairs in linejs order.
    pub(crate) fn attributes(&self) -> Vec<(i32, String)> {
        let b = |v: bool| if v { "true".to_string() } else { "false".to_string() };
        let mut out = Vec::new();
        let mut put = |attr: i32, v: Option<String>| {
            if let Some(v) = v {
                out.push((attr, v));
            }
        };
        put(ProfileAttribute::DISPLAY_NAME, self.display_name.clone());
        put(ProfileAttribute::STATUS_MESSAGE, self.status_message.clone());
        put(ProfileAttribute::PHONETIC_NAME, self.phonetic_name.clone());
        put(ProfileAttribute::MUSIC_PROFILE, self.music_profile.clone());
        put(ProfileAttribute::ALLOW_SEARCH_BY_USERID, self.allow_search_by_userid.map(b));
        put(ProfileAttribute::ALLOW_SEARCH_BY_EMAIL, self.allow_search_by_email.map(b));
        put(ProfileAttribute::HIDDEN_FROM_LIST, self.hidden_from_list.map(b));
        out
    }

    /// The `profileAttributes` map (`map<i32, ProfileContent>`).
    pub(crate) fn attribute_map(&self) -> Value {
        let mut map = Map::new();
        for (attr, value) in self.attributes() {
            let mut content = Map::new();
            content.insert("value".into(), Value::String(value));
            content.insert("meta".into(), Value::Map(Vec::new()));
            map.insert(attr.to_string(), Value::Struct(content));
        }
        Value::Struct(map)
    }
}

impl Client {
    /// The logged-in user's profile (`getProfile`).
    pub async fn get_my_profile(&self) -> Result<Value> {
        self.base.talk().call("getProfile", tv!({"syncReason": "INTERNAL"})).await
    }

    /// Update profile attributes (`updateProfileAttributes`); no-op when
    /// nothing is set.
    pub async fn update_my_profile(&self, update: &MyProfileUpdate) -> Result<()> {
        if update.attributes().is_empty() {
            return Ok(());
        }
        let mut request = Value::object();
        request.insert("profileAttributes", update.attribute_map());
        let mut args = tv!({"reqSeq": self.base.get_reqseq("talk").await?});
        args.insert("request", request);
        self.base.talk().call("updateProfileAttributes", args).await?;
        Ok(())
    }

    pub async fn update_my_display_name(&self, display_name: &str) -> Result<()> {
        self.update_my_profile(&MyProfileUpdate { display_name: Some(display_name.into()), ..Default::default() }).await
    }

    pub async fn update_my_status_message(&self, status_message: &str) -> Result<()> {
        self.update_my_profile(&MyProfileUpdate { status_message: Some(status_message.into()), ..Default::default() })
            .await
    }

    /// Upload a new profile picture (OBS `talk/p/<mid>`). Returns
    /// `(obj_id, obj_hash)`.
    pub async fn upload_my_profile_image(&self, data: Vec<u8>) -> Result<(String, String)> {
        let Some(mid) = self.base.mid() else {
            return Err(Error::internal("NotAuthorized", "uploadMyProfileImage requires a logged-in client"));
        };
        self.upload_profile_object(data, format!("talk/p/{mid}")).await
    }

    /// Upload a new profile background (OBS `myhome/h`). Returns
    /// `(obj_id, obj_hash)`.
    pub async fn upload_my_profile_background(&self, data: Vec<u8>) -> Result<(String, String)> {
        self.upload_profile_object(data, "myhome/h".to_string()).await
    }

    async fn upload_profile_object(&self, data: Vec<u8>, obs_path: String) -> Result<(String, String)> {
        let opts = crate::obs::ObjectUpload {
            data: data.into(),
            o_type: crate::obs::ObjType::Image,
            obs_path: Some(obs_path),
            ..Default::default()
        };
        let up = self.base.obs().upload_object_for_service(opts).await?;
        Ok((up.obj_id, up.obj_hash))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thrift::{encode_struct, Schema, TValue};

    #[test]
    fn attribute_map_encodes() {
        let update = MyProfileUpdate {
            status_message: Some("busy".into()),
            hidden_from_list: Some(true),
            ..Default::default()
        };
        assert_eq!(update.attributes(), vec![(16, "busy".to_string()), (1024, "true".to_string())]);
        let mut request = Value::object();
        request.insert("profileAttributes", update.attribute_map());
        let mut args = tv!({"reqSeq": 1});
        args.insert("request", request);
        let fields = encode_struct(Schema::line(), "updateProfileAttributes_args", &args).unwrap();
        assert_eq!(fields[0], (1, TValue::I32(1)));
        let TValue::Struct(req) = &fields[1].1 else { panic!("request struct") };
        let (_, TValue::Map(_, _, entries)) = &req[0] else { panic!("map") };
        assert_eq!(entries[0].0, TValue::I32(16));
        assert_eq!(entries[0].1.field(1).and_then(TValue::as_str), Some("busy"));
        assert!(MyProfileUpdate::default().attributes().is_empty());
    }
}
