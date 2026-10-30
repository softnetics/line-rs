//! OpenChat (square) ping bot (port of linejs `example/square/ping.ts`).
//!
//! ```sh
//! LINE_AUTH_TOKEN=... cargo run --example square_ping
//! ```

use line_rs::client::{login_with_auth_token, ClientEvent, InitOptions, ListenOptions, ReactionType};
use line_rs::storage::FileStorage;
use line_rs::Device;

#[tokio::main]
async fn main() -> line_rs::Result<()> {
    let token = std::env::var("LINE_AUTH_TOKEN").expect("set LINE_AUTH_TOKEN");
    let init = InitOptions::new(Device::DesktopWin).storage(FileStorage::new("./storage.json")?);
    let client = login_with_auth_token(&token, init).await?;

    let mut events = client.subscribe();
    let _listen = client.listen(ListenOptions { talk: false, square: true, ..Default::default() });
    while let Ok(event) = events.recv().await {
        if let ClientEvent::SquareMessage(message) = event {
            println!("{}", message.text());
            if message.text() == "!ping" {
                message.react(ReactionType::Nice).await?;
                message.reply("pong!").await?;
            }
        }
    }
    Ok(())
}
