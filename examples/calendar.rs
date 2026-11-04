//! Lists LINE Calendar calendars and their events (read-only).
//!
//! Use a throwaway test account, not a primary one.
//!
//! ```sh
//! LINE_AUTH_TOKEN=... cargo run --example calendar
//! # or log in with a QR code (no env vars):
//! cargo run --example calendar
//! ```

use line_rs::Device;
use line_rs::client::calendar::EventTarget;
use line_rs::client::{InitOptions, login_with_auth_token, login_with_qr};
use line_rs::storage::FileStorage;

#[tokio::main]
async fn main() -> line_rs::Result<()> {
    let init =
        InitOptions::new(Device::AndroidSecondary).storage(FileStorage::new("./storage.json")?);
    let client = if let Ok(token) = std::env::var("LINE_AUTH_TOKEN") {
        login_with_auth_token(&token, init).await?
    } else {
        login_with_qr(
            init,
            |url| println!("Scan this QR URL: {url}"),
            |pin| println!("Enter PIN: {pin}"),
        )
        .await?
    };
    println!("logged in as {}", client.mid().unwrap_or_default());

    let calendars = client.base.list_all_calendars().await?;
    println!("\n{} calendar(s):", calendars.len());
    for c in &calendars {
        let kind = if c.chatroom_id.is_empty() {
            "personal".to_string()
        } else {
            format!("chat {}", c.chatroom_id)
        };
        println!("  [{}] {}  ({kind})", c.id, c.name);
    }

    for c in &calendars {
        let events = client
            .base
            .list_events_stream(EventTarget::Calendar(&c.id), 0)
            .await?;
        println!("\n{} — {} event(s):", c.name, events.len());
        for e in &events {
            println!(
                "  {}  {}  [{}..{}]{}",
                e.id,
                e.title,
                e.start_time,
                e.end_time,
                if e.is_all_day { " all-day" } else { "" }
            );
        }
    }
    Ok(())
}
