//! Content-free smoke test for the LINE Calendar gRPC path: prints only
//! counts and which fields were present, never names/titles/ids.
//!
//! ```sh
//! LINE_AUTH_TOKEN=... cargo run --example calendar_smoke
//! ```

use line_rs::Device;
use line_rs::client::calendar::EventTarget;
use line_rs::client::{InitOptions, login_with_auth_token};
use line_rs::storage::MemoryStorage;

#[tokio::main]
async fn main() -> line_rs::Result<()> {
    let token = std::env::var("LINE_AUTH_TOKEN").expect("set LINE_AUTH_TOKEN");
    let init = InitOptions::new(Device::AndroidSecondary).storage(MemoryStorage::new());
    let client = login_with_auth_token(&token, init).await?;
    println!("login ok: {}", client.mid().is_some());

    // Chat-targeted path (the feature's real entry point: group calendars).
    // LINE_CHAT_IDS is a comma-separated list; nothing identifying is printed.
    if let Ok(chats) = std::env::var("LINE_CHAT_IDS") {
        for (i, chat) in chats.split(',').filter(|s| !s.is_empty()).enumerate() {
            match client
                .base
                .list_events_stream(EventTarget::Chat(chat), 0)
                .await
            {
                Ok(events) => {
                    let with_title = events.iter().filter(|e| !e.title.is_empty()).count();
                    let with_start = events.iter().filter(|e| e.start_time != 0).count();
                    println!(
                        "chat #{i}: ok, {} event(s), {with_title} titled, {with_start} with start time",
                        events.len()
                    );
                }
                Err(e) => println!("chat #{i}: {e}"),
            }
        }
    }

    match client.base.list_all_calendars().await {
        Ok(calendars) => {
            let personal = calendars
                .iter()
                .filter(|c| c.chatroom_id.is_empty())
                .count();
            println!(
                "list_all_calendars ok: {} calendar(s) ({personal} personal, {} chat)",
                calendars.len(),
                calendars.len() - personal
            );
        }
        Err(e) => println!("list_all_calendars failed: {e}"),
    }
    Ok(())
}
