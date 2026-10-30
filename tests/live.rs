//! Network smoke tests against the real LINE API (no credentials needed).
//! Run with `cargo test --test live -- --ignored`.

use line_rs::{BaseClient, Device};

#[tokio::test]
#[ignore = "hits the real LINE server"]
async fn rsa_key_info() {
    let base = BaseClient::new(Device::DesktopWin).unwrap();
    let key = base.get_rsa_key_info(0).await.unwrap();
    assert!(key["keynm"].as_str().is_some_and(|s| !s.is_empty()), "{key}");
    assert!(key["nvalue"].as_str().is_some_and(|s| s.len() >= 256), "{key}");
}

#[tokio::test]
#[ignore = "hits the real LINE server"]
async fn sqr_create_session() {
    let base = BaseClient::new(Device::DesktopWin).unwrap();
    let sqr = base.create_session().await.unwrap();
    assert!(!sqr.is_empty());
    let qr = base.create_qr_code_for_secure(&sqr).await.unwrap();
    assert!(qr["1"].as_str().is_some_and(|u| u.starts_with("https://")), "{qr}");
}
