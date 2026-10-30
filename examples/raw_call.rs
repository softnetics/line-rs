//! Calling any IDL method directly with the low-level `BaseClient`.
//!
//! ```sh
//! LINE_AUTH_TOKEN=... cargo run --example raw_call
//! ```

use line_rs::login::LoginOption;
use line_rs::{tv, BaseClient, Device};

#[tokio::main]
async fn main() -> line_rs::Result<()> {
    let token = std::env::var("LINE_AUTH_TOKEN").expect("set LINE_AUTH_TOKEN");
    let base = BaseClient::new(Device::DesktopWin)?;
    base.login(LoginOption::token(token)).await?;

    // Args are the fields of the IDL struct `getAllChatMids_args`.
    let mids = base.talk().call("getAllChatMids", tv!({"request": {"withMemberChats": true}, "syncReason": "INTERNAL"})).await?;
    let chat_mids: Vec<String> =
        mids["memberChatMids"].as_list().unwrap_or_default().iter().filter_map(|v| v.as_str().map(String::from)).collect();
    println!("{} chats", chat_mids.len());

    let chats = base.talk().call("getChats", tv!({"request": {"chatMids": chat_mids, "withMembers": true}})).await?;
    for chat in chats["chats"].as_list().unwrap_or_default() {
        println!("{}  {}", chat["chatMid"].as_str().unwrap_or("?"), chat["chatName"].as_str().unwrap_or(""));
    }
    Ok(())
}
