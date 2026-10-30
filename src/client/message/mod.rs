//! Message objects and content helpers (linejs `features/message`).

mod square;
mod talk;
pub mod types;
pub mod utils;

pub use square::{SquareMessage, SquareThreadMessage};
pub use talk::TalkMessage;
pub use types::{
    AppLinkInfo, CallInfo, ChatEventInfo, ContactMeta, Decoration, Emoji, FileInfo, FlexMeta, GiftInfo, MentionTarget, Mid,
    PostNotificationInfo, ReactionType,
};
pub use utils::{build_text_decorations, sticker_metadata, MessageInput};
