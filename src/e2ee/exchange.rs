//! E2EE key-exchange / key-maintenance calls and operations.
//!
//! The `TalkService` methods below are absent from the bundled linejs IDL;
//! their argument structs were read off the generated
//! `TalkService_*_pargs::write` functions of LINE macOS 26.5.0 (field
//! name, thrift type and id per `writeFieldBegin`), and their result structs
//! off the matching `*_presult::read` (exception field 1 only, i.e. `void`):
//!
//! * `requestE2EEKeyExchange(1: i32 reqSeq, 2: string temporaryPublicKey,
//!   3: E2EEPublicKey publicKey, 4: string verifier)`
//! * `respondE2EEKeyExchange(1: i32 reqSeq, 2: string encryptedKeyChain,
//!   3: string hashKeyChain)`
//! * `removeE2EEPublicKey(2: E2EEPublicKey publicKey)`
//!
//! (`E2EEPublicKey` is `Pb1_C13097n4` in the IDL; `registerE2EEPublicKey`
//! serialises it with the same writer.) Derived from static RE of LINE macOS
//! 26.5.0; unverified against live server. The semantics of the key
//! exchange (how the temporary key, verifier and key chain are produced) and
//! of the `NOTIFIED_E2EE_*` operation params were not recovered, so this
//! module only exposes the raw calls and classifies the operations.

use crate::base::BaseClient;
use crate::error::Result;
use crate::thrift::{Schema, Value};

/// E2EE-related talk operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum E2eeOpKind {
    /// `REGISTER_E2EE_PUBLICKEY` (72): one of our devices registered a key.
    RegisterPublicKey,
    /// `UPDATE_PUBLICKEYCHAIN` / `NOTIFIED_UPDATE_PUBLICKEYCHAIN` (66/67).
    UpdatePublicKeyChain,
    /// `NOTIFIED_E2EE_KEY_EXCHANGE_REQ` (73).
    KeyExchangeRequest,
    /// `NOTIFIED_E2EE_KEY_EXCHANGE_RESP` (74).
    KeyExchangeResponse,
    /// `NOTIFIED_E2EE_MESSAGE_RESEND_REQ` (75).
    MessageResendRequest,
    /// `NOTIFIED_E2EE_MESSAGE_RESEND_RESP` (76).
    MessageResendResponse,
    /// `NOTIFIED_E2EE_KEY_UPDATE` (77).
    KeyUpdate,
}

/// An E2EE operation with its untyped params.
#[derive(Debug, Clone, PartialEq)]
pub struct E2eeOperation {
    pub kind: E2eeOpKind,
    pub revision: Option<i64>,
    pub param1: Option<String>,
    pub param2: Option<String>,
    pub param3: Option<String>,
}

impl E2eeOperation {
    /// Classify a talk `Operation` (type given by name or number).
    pub fn from_op(op: &Value) -> Option<Self> {
        let name = match &op["type"] {
            Value::String(s) => s.clone(),
            Value::Int(i) => Schema::line().get_enum("OpType")?.name_of(*i as i32)?.to_string(),
            _ => return None,
        };
        let kind = match name.as_str() {
            "REGISTER_E2EE_PUBLICKEY" => E2eeOpKind::RegisterPublicKey,
            "UPDATE_PUBLICKEYCHAIN" | "NOTIFIED_UPDATE_PUBLICKEYCHAIN" => E2eeOpKind::UpdatePublicKeyChain,
            "NOTIFIED_E2EE_KEY_EXCHANGE_REQ" => E2eeOpKind::KeyExchangeRequest,
            "NOTIFIED_E2EE_KEY_EXCHANGE_RESP" => E2eeOpKind::KeyExchangeResponse,
            "NOTIFIED_E2EE_MESSAGE_RESEND_REQ" => E2eeOpKind::MessageResendRequest,
            "NOTIFIED_E2EE_MESSAGE_RESEND_RESP" => E2eeOpKind::MessageResendResponse,
            "NOTIFIED_E2EE_KEY_UPDATE" => E2eeOpKind::KeyUpdate,
            _ => return None,
        };
        let param = |k: &str| op[k].as_str().map(str::to_string);
        Some(Self {
            kind,
            revision: op["revision"].as_i64(),
            param1: param("param1"),
            param2: param("param2"),
            param3: param("param3"),
        })
    }
}

impl BaseClient {
    /// `requestE2EEKeyExchange`. `public_key` is an `E2EEPublicKey`
    /// (`Pb1_C13097n4`: `version`, `keyId`, `keyData`, `createdTime`).
    pub async fn request_e2ee_key_exchange(
        &self,
        temporary_public_key: &[u8],
        public_key: Value,
        verifier: &[u8],
    ) -> Result<()> {
        let mut args = crate::tv!({"reqSeq": self.get_reqseq("talk").await?});
        args.insert("temporaryPublicKey", Value::Binary(temporary_public_key.to_vec()));
        args.insert("publicKey", public_key);
        args.insert("verifier", Value::Binary(verifier.to_vec()));
        self.talk().call("requestE2EEKeyExchange", args).await.map(drop)
    }

    /// `respondE2EEKeyExchange`.
    pub async fn respond_e2ee_key_exchange(&self, encrypted_key_chain: &[u8], hash_key_chain: &[u8]) -> Result<()> {
        let mut args = crate::tv!({"reqSeq": self.get_reqseq("talk").await?});
        args.insert("encryptedKeyChain", Value::Binary(encrypted_key_chain.to_vec()));
        args.insert("hashKeyChain", Value::Binary(hash_key_chain.to_vec()));
        self.talk().call("respondE2EEKeyExchange", args).await.map(drop)
    }

    /// `removeE2EEPublicKey` for one of our registered keys.
    pub async fn remove_e2ee_public_key(&self, public_key: Value) -> Result<()> {
        let mut args = Value::object();
        args.insert("publicKey", public_key);
        self.talk().call("removeE2EEPublicKey", args).await.map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thrift::{encode_struct, TValue};

    #[test]
    fn classifies_ops_by_name_and_number() {
        let op = crate::tv!({"type": 73, "revision": 9, "param1": "u1", "param2": "x"});
        let e = E2eeOperation::from_op(&op).unwrap();
        assert_eq!(e.kind, E2eeOpKind::KeyExchangeRequest);
        assert_eq!(e.revision, Some(9));
        assert_eq!(e.param1.as_deref(), Some("u1"));
        assert_eq!(e.param3, None);
        let op = crate::tv!({"type": "NOTIFIED_E2EE_MESSAGE_RESEND_RESP"});
        assert_eq!(E2eeOperation::from_op(&op).unwrap().kind, E2eeOpKind::MessageResendResponse);
        assert_eq!(E2eeOperation::from_op(&crate::tv!({"type": 77})).unwrap().kind, E2eeOpKind::KeyUpdate);
        assert_eq!(E2eeOperation::from_op(&crate::tv!({"type": 66})).unwrap().kind, E2eeOpKind::UpdatePublicKeyChain);
        assert!(E2eeOperation::from_op(&crate::tv!({"type": "RECEIVE_MESSAGE"})).is_none());
        assert!(E2eeOperation::from_op(&crate::tv!({})).is_none());
    }

    /// The IDL entries encode the field ids/types read from the binary.
    #[test]
    fn idl_args_match_recovered_layout() {
        let schema = Schema::line();
        let mut args = crate::tv!({"reqSeq": 3});
        args.insert("temporaryPublicKey", Value::Binary(vec![1, 2]));
        let mut pk = crate::tv!({"version": 1, "keyId": 7, "createdTime": 0});
        pk.insert("keyData", Value::Binary(vec![9; 32]));
        args.insert("publicKey", pk.clone());
        args.insert("verifier", Value::Binary(vec![5]));
        let fields = encode_struct(schema, "requestE2EEKeyExchange_args", &args).unwrap();
        let ids: Vec<i16> = fields.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, [1, 2, 3, 4]);
        assert!(matches!(fields[0].1, TValue::I32(3)));
        assert!(matches!(&fields[1].1, TValue::Binary(b) if b == &[1, 2]));
        assert!(matches!(fields[2].1, TValue::Struct(_)));

        let mut args = crate::tv!({"reqSeq": 1});
        args.insert("encryptedKeyChain", Value::Binary(vec![1]));
        args.insert("hashKeyChain", Value::Binary(vec![2]));
        let fields = encode_struct(schema, "respondE2EEKeyExchange_args", &args).unwrap();
        let ids: Vec<i16> = fields.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, [1, 2, 3]);

        let mut args = Value::object();
        args.insert("publicKey", pk);
        let fields = encode_struct(schema, "removeE2EEPublicKey_args", &args).unwrap();
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].0, 2);
    }
}
