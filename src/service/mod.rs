//! Thrift services. Every LINE service is reachable through a typed
//! accessor on [`BaseClient`] (e.g. [`BaseClient::talk`]); any IDL method can be
//! invoked with [`Service::call`].
//!
//! Each per-service module mirrors the linejs service class: the plain
//! pass-through methods (`fn x(&self, args: impl Into<Value>)`, args encoded as
//! the IDL `x_args` struct) are generated from the linejs sources between
//! `// <generated pass-through>` markers, and methods with custom logic or
//! convenience parameters are hand-ported above them, taking option structs.
//!
//! The album REST API ([`moa::MoaService`]) is not thrift and lives on its own.

pub mod access_token_refresh;
pub mod account_auth_factor_eap_connect;
pub mod auth;
pub mod bot_external;
pub mod buddy;
pub mod call;
pub mod channel;
pub mod chat_app;
pub mod coin;
pub mod device_attestation;
pub mod e2ee_key_backup;
pub mod home_safety_check;
pub mod liff;
pub mod livetalk;
pub mod moa;
pub mod multi_profile;
pub mod oa_chat;
pub mod oa_membership;
pub mod password_update;
pub mod premium_font;
pub mod premium_product_definition;
pub mod premium_status;
pub mod primary_account_init;
pub mod primary_account_re_login;
pub mod primary_account_smart_switch_restore;
pub mod primary_account_smart_switch_restore_preparation;
pub mod primary_qr_code_migration_long_polling;
pub mod primary_qr_code_migration_preparation;
pub mod primary_seamless_login;
pub mod pwless_primary_registration;
pub mod relation;
pub mod secondary_pwless_login;
pub mod secondary_seamless_login;
pub mod settings;
pub mod shop;
pub mod shop_auth;
pub mod shop_collection;
pub mod square;
pub mod square_bot;
pub mod talk;

use std::ops::Deref;

use crate::base::BaseClient;
use crate::error::Result;
use crate::request::{Parse, RequestOptions};
use crate::thrift::schema::Named;
use crate::thrift::{Protocol, Schema, TValue, TypeRef, Value};

/// A service endpoint bound to a client.
#[derive(Clone, Copy)]
pub struct Service<'a> {
    pub client: &'a BaseClient,
    pub path: &'static str,
    pub protocol: Protocol,
}

impl<'a> Service<'a> {
    pub const fn new(client: &'a BaseClient, path: &'static str, protocol: Protocol) -> Self {
        Self { client, path, protocol }
    }

    /// Call `method` with args encoded as the IDL struct `<method>_args`;
    /// returns the decoded `success` value.
    pub async fn call(&self, method: &str, args: impl Into<Value>) -> Result<Value> {
        self.client.call(method, args, self.path, self.protocol).await
    }

    /// [`call`](Self::call) with adjusted [`RequestOptions`] (path, parse mode,
    /// headers, timeout).
    pub async fn call_with(
        &self,
        method: &str,
        args: impl Into<Value>,
        opts: impl FnOnce(RequestOptions<'_>) -> RequestOptions<'_>,
    ) -> Result<Value> {
        self.client.call_with(method, args, opts(RequestOptions::new(self.path, self.protocol))).await
    }

    /// Send pre-encoded args (linejs `NestedArray` requests) to this
    /// service, decoding the reply with `parse`.
    pub async fn request(&self, method: &str, args: Vec<(i16, TValue)>, parse: Parse<'_>) -> Result<Value> {
        self.client.request(args, method, RequestOptions::new(self.path, self.protocol).parse(parse)).await
    }
}

macro_rules! services {
    ($($(#[$m:meta])* $ty:ident => $acc:ident, $path:literal, $proto:ident;)*) => {$(
        $(#[$m])*
        #[derive(Clone, Copy)]
        pub struct $ty<'a>(pub Service<'a>);

        impl<'a> Deref for $ty<'a> {
            type Target = Service<'a>;
            fn deref(&self) -> &Service<'a> { &self.0 }
        }

        impl BaseClient {
            #[doc = concat!("`", $path, "`")]
            pub fn $acc(&self) -> $ty<'_> { $ty(Service::new(self, $path, Protocol::$proto)) }
        }
    )*};
}

services! {
    AccessTokenRefreshService => access_token_refresh, "/EXT/auth/tokenrefresh/v1", Compact;
    AccountAuthFactorEapConnectService => account_auth_factor_eap_connect, "/ACCT/authfactor/eap/v1", Compact;
    AuthService => auth, "/AS4", Compact;
    BotExternalService => bot_external, "/BOTE", Compact;
    BuddyService => buddy, "/BUDDY4", Compact;
    CallService => call_service, "/V4", Compact;
    ChannelService => channel, "/CH4", Compact;
    ChatAppService => chat_app, "/CAPP1", Compact;
    CoinService => coin, "/COIN4", Compact;
    DeviceAttestationService => device_attestation, "/EXT/auth/feature-user/api/primary/device-attestation", Compact;
    E2EEKeyBackupService => e2ee_key_backup, "/EKBS4", Compact;
    HomeSafetyCheckService => home_safety_check, "/EXT/home/safety-check/safety-check", Compact;
    LiffService => liff, "/LIFF1", Compact;
    MultiProfileService => multi_profile, "/PFS4", Compact;
    OaChatService => oa_chat, "/ext/oachat/api/", Compact;
    OaMembershipService => oa_membership, "/EXT/bot/oafan", Compact;
    PasswordUpdateService => password_update, "/EXT/auth/feature-user/api/primary/password/update", Compact;
    PremiumFontService => premium_font, "/EXT/talk/asset-meta-server/thrift/fontService", Compact;
    PremiumProductDefinitionService => premium_product_definition, "/EXT/line-premium/common/thrift/product-definition", Compact;
    PremiumStatusService => premium_status, "/EXT/line-premium/common/thrift/status", Compact;
    PrimaryAccountInitService => primary_account_init, "/acct/pais/v1", Compact;
    PrimaryAccountReLoginService => primary_account_re_login, "/ext/auth/feature-guest/thrift/account/v1/relogin/primary", Compact;
    PrimaryAccountSmartSwitchRestorePreparationService => primary_account_smart_switch_restore_preparation, "/EXT/auth/feature-user/thrift/account/v1/restore/smart-switch/prepare", Compact;
    PrimaryAccountSmartSwitchRestoreService => primary_account_smart_switch_restore, "/ext/auth/feature-guest/thrift/account/v1/restore/smart-switch", Compact;
    PrimaryQrCodeMigrationLongPollingService => primary_qr_code_migration_long_polling, "/EXT/auth/feature-user/lp/api/primary/mig/qr", Compact;
    PrimaryQrCodeMigrationPreparationService => primary_qr_code_migration_preparation, "/EXT/auth/feature-user/api/primary/mig/qr/prepare", Compact;
    PrimarySeamlessLoginService => primary_seamless_login, "/EXT/auth/feature-user/api/secondary/login/seamless", Compact;
    PwlessPrimaryRegistrationService => pwless_primary_registration, "/ACCT/authfactor/pwless/v1", Compact;
    RelationService => relation, "/RE4", Compact;
    SecondaryPwlessLoginService => secondary_pwless_login, "/acct/lgn/secpwless/v1", Compact;
    SecondarySeamlessLoginService => secondary_seamless_login, "/ext/auth/feature-guest/api/secondary/login/seamless", Compact;
    SettingsService => settings, "/US4", Compact;
    ShopAuthService => shop_auth, "/SHOPA", Compact;
    ShopCollectionService => shop_collection, "/ext/sapi/sapic", Compact;
    ShopService => shop, "/TSHOP4", Compact;
    SquareBotService => square_bot, "/BP1", Compact;
    SquareLiveTalkService => livetalk, "/SQLV1", Compact;
    SquareService => square, "/SQ1", Compact;
    TalkService => talk, "/S4", Compact;
}

/// Rename a schema-less value (struct fields keyed by numeric id, as returned
/// by [`Parse::Raw`]) as the IDL type `ty`: field ids become names and enum
/// numbers become variant names. Unknown fields keep their id keys.
pub fn rename_raw(schema: &Schema, ty: &TypeRef, value: Value) -> Value {
    match (ty, value) {
        (TypeRef::List(inner) | TypeRef::Set(inner), Value::List(items)) => {
            Value::List(items.into_iter().map(|v| rename_raw(schema, inner, v)).collect())
        }
        (TypeRef::Map(kt, vt), Value::Map(entries)) => Value::Map(
            entries.into_iter().map(|(k, v)| (rename_raw(schema, kt, k), rename_raw(schema, vt, v))).collect(),
        ),
        (TypeRef::Named(name), value) => match (schema.resolve(name), value) {
            (Some(Named::Struct(def)), Value::Struct(map)) => Value::Struct(
                map.into_iter()
                    .map(|(k, v)| match k.parse::<i16>().ok().and_then(|id| def.field_by_id(id)) {
                        Some(f) => (f.name.clone(), rename_raw(schema, &f.ty, v)),
                        None => (k, v),
                    })
                    .collect(),
            ),
            (Some(Named::Enum(def)), Value::Int(i)) => {
                def.name_of(i as i32).map(|n| Value::String(n.to_string())).unwrap_or(Value::Int(i))
            }
            (_, v) => v,
        },
        (_, v) => v,
    }
}

#[cfg(test)]
pub(crate) mod test_util {
    //! Helpers for asserting encoded request args without network access.

    use crate::thrift::{Schema, TValue, Value, encode_struct};

    /// Encode `value` as the IDL struct `name` (panics on schema errors).
    pub fn encode(name: &str, value: &Value) -> Vec<(i16, TValue)> {
        encode_struct(Schema::line(), name, value).unwrap_or_else(|e| panic!("encode {name}: {e}"))
    }

    /// Field `id` of an encoded struct.
    pub fn field(fields: &[(i16, TValue)], id: i16) -> Option<&TValue> {
        fields.iter().find(|(fid, _)| *fid == id).map(|(_, v)| v)
    }

    /// Walk nested struct fields by id.
    pub fn path<'a>(fields: &'a [(i16, TValue)], ids: &[i16]) -> Option<&'a TValue> {
        let (first, rest) = ids.split_first()?;
        let mut cur = field(fields, *first)?;
        for id in rest {
            cur = cur.field(*id)?;
        }
        Some(cur)
    }

    pub fn s(v: &str) -> TValue {
        TValue::string(v)
    }

    /// A [`BaseClient`](crate::BaseClient) with in-memory storage (no network).
    pub fn client() -> crate::BaseClient {
        crate::BaseClient::new(crate::Device::IosIpad).unwrap()
    }
}
