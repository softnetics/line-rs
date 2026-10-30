//! Recovery from E2EE `sendMessage` failures.
//!
//! The desktop client's `ChatSecurityServiceImpl::handleE2EEErrorCodeInternal`
//! switches on the `ErrorCode` of a failed send:
//!
//! | code | action |
//! |---|---|
//! | `E2EE_RETRY_ENCRYPT` (82), `E2EE_UPDATE_RECEIVER_KEY` (84), `E2EE_INVALID_VERSION` (86) | re-negotiate the receiver key, re-encrypt, resend |
//! | `E2EE_RECREATE_GROUP_KEY` (99) | queue the message, `recreateGroupKey` (`getLastE2EEPublicKeys` → `registerE2EEGroupKey`), re-encrypt |
//! | `E2EE_RETRY_PLAIN` (98), `E2EE_RECEIVER_DISABLED` (88), `E2EE_RECEIVER_NOT_ALLOWED` (90), `E2EE_GROUP_TOO_MANY_MEMBERS` (100) | turn E2EE off for the chat, resend plain |
//!
//! and gives up on a request once it has been handled five times. Other
//! codes (e.g. `E2EE_UPDATE_SENDER_KEY`, `E2EE_SENDER_DISABLED`) are not
//! retried here: their client-side handling could not be pinned down.
//!
//! Derived from static RE of LINE macOS 26.5.0; unverified against live
//! server.

use crate::base::BaseClient;
use crate::error::Result;

/// Upper bound on `sendMessage` attempts for one message (the client stops
/// after handling the same request five times).
pub const MAX_E2EE_SEND_ATTEMPTS: u32 = 5;

/// What to do before resending after an E2EE send error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum E2eeSendRecovery {
    /// Fetch the receiver's current key (or the group's last key) and
    /// encrypt again.
    Reencrypt,
    /// Register a new group shared key, then encrypt again.
    RecreateGroupKey,
    /// Send without E2EE.
    RetryPlain,
}

/// The recovery for an `ErrorCode` given by name (as decoded from a
/// `TalkException`) or number; `None` when the error should be returned.
pub fn send_recovery(code: &str) -> Option<E2eeSendRecovery> {
    use E2eeSendRecovery::*;
    Some(match code {
        "E2EE_RETRY_ENCRYPT" | "82" | "E2EE_UPDATE_RECEIVER_KEY" | "84" | "E2EE_INVALID_VERSION" | "86" => Reencrypt,
        "E2EE_RECREATE_GROUP_KEY" | "99" => RecreateGroupKey,
        "E2EE_RETRY_PLAIN" | "98" | "E2EE_RECEIVER_DISABLED" | "88" | "E2EE_RECEIVER_NOT_ALLOWED" | "90"
        | "E2EE_GROUP_TOO_MANY_MEMBERS" | "100" => RetryPlain,
        _ => return None,
    })
}

impl BaseClient {
    /// Side effects needed before resending to `to` after `recovery`: a new
    /// group key for [`E2eeSendRecovery::RecreateGroupKey`] on a group or
    /// room. Re-encryption itself always negotiates the receiver's current
    /// key, so nothing is cached that would need dropping.
    pub async fn prepare_e2ee_resend(&self, to: &str, recovery: E2eeSendRecovery) -> Result<()> {
        if recovery == E2eeSendRecovery::RecreateGroupKey && matches!(Self::get_to_type(to), Some(1 | 2)) {
            self.try_register_e2ee_group_key(to).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_codes_by_name_and_number() {
        assert_eq!(send_recovery("E2EE_RETRY_ENCRYPT"), Some(E2eeSendRecovery::Reencrypt));
        assert_eq!(send_recovery("84"), Some(E2eeSendRecovery::Reencrypt));
        assert_eq!(send_recovery("E2EE_INVALID_VERSION"), Some(E2eeSendRecovery::Reencrypt));
        assert_eq!(send_recovery("E2EE_RECREATE_GROUP_KEY"), Some(E2eeSendRecovery::RecreateGroupKey));
        assert_eq!(send_recovery("E2EE_RETRY_PLAIN"), Some(E2eeSendRecovery::RetryPlain));
        assert_eq!(send_recovery("100"), Some(E2eeSendRecovery::RetryPlain));
        assert_eq!(send_recovery("E2EE_UPDATE_SENDER_KEY"), None);
        assert_eq!(send_recovery("E2EE_SENDER_DISABLED"), None);
        assert_eq!(send_recovery("NOT_FOUND"), None);
    }
}
