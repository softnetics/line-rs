//! Recovery from E2EE `sendMessage` failures.
//!
//! Every `E2EE_*` error of a send is handled by its code, and then the same
//! operation is sent again with the same `reqSeq`:
//!
//! | code | action |
//! |---|---|
//! | `E2EE_RETRY_ENCRYPT` (82), `E2EE_UPDATE_RECEIVER_KEY` (84), `E2EE_INVALID_VERSION` (86), `E2EE_RECEIVER_DISABLED` (88), `E2EE_RECEIVER_NOT_ALLOWED` (90) | look the chat's key up again (`negotiateE2EEPublicKey` / `getLastE2EEGroupSharedKey`) and resend as the new state says |
//! | `E2EE_UPDATE_SENDER_KEY` (83) | drop the cached own key, load (or register) the current one, resend |
//! | `E2EE_SENDER_DISABLED` (87), `E2EE_SENDER_NOT_ALLOWED` (89) | turn E2EE off for the sender, resend plain |
//! | `E2EE_RETRY_PLAIN` (98), `E2EE_GROUP_TOO_MANY_MEMBERS` (100) | mark the chat plain, resend plain |
//! | `E2EE_RECREATE_GROUP_KEY` (99) | register a new group shared key, resend |
//!
//! `E2EE_INVALID_PROTOCOL` (81), `E2EE_INVALID_ARGUMENT` (85) and
//! `E2EE_RESEND_FAIL` (91) are not recovered. The send loop itself is
//! [`BaseClient::send_with_e2ee_recovery`].

use crate::base::BaseClient;
use crate::error::Result;

/// Upper bound on `sendMessage` attempts for one message: the first send and
/// two resends, counted per message.
pub const MAX_E2EE_SEND_ATTEMPTS: u32 = 3;

/// What to do before resending after an E2EE send error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum E2eeSendRecovery {
    /// Look up the receiver's current key (or the group's last key) again
    /// and send as the refreshed chat state says.
    Reencrypt,
    /// Register a new group shared key, then encrypt again.
    RecreateGroupKey,
    /// Mark the chat plain and send without E2EE.
    RetryPlain,
    /// Our own key is outdated: reload it, then encrypt again.
    RefreshSenderKey,
    /// The server does not take E2EE from this account (sender disabled or
    /// not allowed): send this message plain. The chat is not marked.
    SendPlain,
}

/// The recovery for an `ErrorCode` given by name (as decoded from a
/// `TalkException`) or number; `None` when the error should be returned.
pub fn send_recovery(code: &str) -> Option<E2eeSendRecovery> {
    use E2eeSendRecovery::*;
    Some(match code {
        "E2EE_RETRY_ENCRYPT"
        | "82"
        | "E2EE_UPDATE_RECEIVER_KEY"
        | "84"
        | "E2EE_INVALID_VERSION"
        | "86"
        | "E2EE_RECEIVER_DISABLED"
        | "88"
        | "E2EE_RECEIVER_NOT_ALLOWED"
        | "90" => Reencrypt,
        "E2EE_UPDATE_SENDER_KEY" | "83" => RefreshSenderKey,
        "E2EE_SENDER_DISABLED" | "87" | "E2EE_SENDER_NOT_ALLOWED" | "89" => SendPlain,
        "E2EE_RECREATE_GROUP_KEY" | "99" => RecreateGroupKey,
        "E2EE_RETRY_PLAIN" | "98" | "E2EE_GROUP_TOO_MANY_MEMBERS" | "100" => RetryPlain,
        _ => return None,
    })
}

impl BaseClient {
    /// Side effects needed before resending to `to` after `recovery`: a new
    /// group key for [`E2eeSendRecovery::RecreateGroupKey`] on a group or
    /// room, the plain mark for [`E2eeSendRecovery::RetryPlain`], a reloaded
    /// own key for [`E2eeSendRecovery::RefreshSenderKey`], and a fresh chat
    /// state for [`E2eeSendRecovery::Reencrypt`].
    /// [`Self::send_with_e2ee_recovery`] does this itself.
    pub async fn prepare_e2ee_resend(&self, to: &str, recovery: E2eeSendRecovery) -> Result<()> {
        let e2ee = self.e2ee();
        match recovery {
            E2eeSendRecovery::RecreateGroupKey if matches!(Self::get_to_type(to), Some(1 | 2)) => {
                e2ee.recreate_group_state(to).await?;
            }
            E2eeSendRecovery::RecreateGroupKey | E2eeSendRecovery::Reencrypt => {
                e2ee.refresh_chat_state(to).await?;
            }
            E2eeSendRecovery::RetryPlain => e2ee.mark_chat_plain(to).await?,
            E2eeSendRecovery::RefreshSenderKey => e2ee.refresh_self_key().await?,
            E2eeSendRecovery::SendPlain => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_codes_by_name_and_number() {
        use E2eeSendRecovery::*;
        for (code, want) in [
            ("E2EE_RETRY_ENCRYPT", Some(Reencrypt)),
            ("84", Some(Reencrypt)),
            ("E2EE_INVALID_VERSION", Some(Reencrypt)),
            ("E2EE_RECEIVER_DISABLED", Some(Reencrypt)),
            ("90", Some(Reencrypt)),
            ("E2EE_UPDATE_SENDER_KEY", Some(RefreshSenderKey)),
            ("E2EE_SENDER_DISABLED", Some(SendPlain)),
            ("89", Some(SendPlain)),
            ("E2EE_RECREATE_GROUP_KEY", Some(RecreateGroupKey)),
            ("E2EE_RETRY_PLAIN", Some(RetryPlain)),
            ("100", Some(RetryPlain)),
            ("E2EE_INVALID_PROTOCOL", None),
            ("E2EE_RESEND_FAIL", None),
            ("NOT_FOUND", None),
        ] {
            assert_eq!(send_recovery(code), want, "{code}");
        }
    }
}
