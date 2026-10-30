//! Talk ping bot (port of linejs `example/talk/ping.ts`).
//!
//! ```sh
//! LINE_EMAIL=... LINE_PASSWORD=... cargo run --example ping
//! # or reuse a token:
//! LINE_AUTH_TOKEN=... cargo run --example ping
//! # or log in with a QR code (no env vars)
//! ```

use line_rs::client::{login_with_auth_token, login_with_password, login_with_qr, ClientEvent, InitOptions, ListenOptions, ReactionType};
use line_rs::storage::FileStorage;
use line_rs::Device;

#[tokio::main]
async fn main() -> line_rs::Result<()> {
    let init = InitOptions::new(Device::DesktopWin).storage(FileStorage::new("./storage.json")?);
    let client = if let Ok(token) = std::env::var("LINE_AUTH_TOKEN") {
        login_with_auth_token(&token, init).await?
    } else if let (Ok(email), Ok(password)) = (std::env::var("LINE_EMAIL"), std::env::var("LINE_PASSWORD")) {
        login_with_password(init, &email, &password, None, |pin| println!("Enter PIN on your phone: {pin}")).await?
    } else {
        login_with_qr(init, |url| println!("Scan this QR URL: {url}"), |pin| println!("Enter PIN: {pin}")).await?
    };
    println!("logged in as {}", client.mid().unwrap_or_default());
    if let Some(token) = client.auth_token() {
        println!("auth token (reuse with LINE_AUTH_TOKEN): {token}");
    }

    let mut events = client.subscribe();
    let _listen = client.listen(ListenOptions { talk: true, square: false, ..Default::default() });
    while let Ok(event) = events.recv().await {
        if let ClientEvent::Message(message) = event {
            println!("{}", message.text());
            if message.text() == "!ping" {
                message.react(ReactionType::Nice).await?;
                message.reply("pong!").await?;
            }
        }
    }
    Ok(())
}
