# line-rs

A Rust port of [LINEJS](https://github.com/evex-dev/linejs), an unofficial
client library for LINE. It covers login (QR code, email and password, auth
token), Letter Sealing (LINE's end-to-end encryption), chats and OpenChat,
LEGY push events, media upload and download, timeline posts, LIFF, and every
Thrift service in LINE's IDL.

It's the LINE backend of [LAAB](https://github.com/softnetics/laab), a LINE
client for macOS.

> [!WARNING]
> This is not an official LINE product. Using an unofficial client can break
> LINE's terms of service, and LINE can restrict or ban the account. Try it
> on an account you can afford to lose.

## Quick start

line-rs is not on crates.io. Add it from git (and pin a `rev` if you want
reproducible builds). It needs Rust 1.88 or newer.

```toml
[dependencies]
line-rs = { git = "https://github.com/softnetics/line-rs" }
tokio = { version = "1", features = ["full"] }
```

```rust
use line_rs::client::{login_with_qr, ClientEvent, InitOptions, ListenOptions, ReactionType};
use line_rs::storage::FileStorage;
use line_rs::Device;

#[tokio::main]
async fn main() -> line_rs::Result<()> {
    let init = InitOptions::new(Device::DesktopWin).storage(FileStorage::new("./storage.json")?);
    let client = login_with_qr(init, |url| println!("scan: {url}"), |pin| println!("PIN: {pin}")).await?;

    let mut events = client.subscribe();
    let _listen = client.listen(ListenOptions::default());
    while let Ok(event) = events.recv().await {
        if let ClientEvent::Message(m) = event {
            if m.text() == "!ping" {
                m.react(ReactionType::Nice).await?;
                m.reply("pong!").await?;
            }
        }
    }
    Ok(())
}
```

This prints a URL to open as a QR code and scan with LINE on your phone,
then a PIN to enter there. After that it answers every `!ping` with a
reaction and "pong!". The session is saved in `storage.json`, so keep that
file private.

[`examples/`](examples) has `ping`, `square_ping`, `raw_call`, and `test_ui`,
a local dashboard for the live tests (see
[`tests/LIVE_TESTING.md`](tests/LIVE_TESTING.md)).

## Modules

| Module | In LINEJS | What it does |
|---|---|---|
| `client` | `@evex/linejs` (`client/`) | `Client`, `TalkMessage`, `SquareMessage`, `Chat`, `User`, `Square`, `SquareChat`, LIFF, VOOM, profile |
| `base` | `@evex/linejs/base` | `BaseClient`: config, token, storage, events, request pipeline |
| `login` | `base/login` | QR (SQR v1, ForSecure v2), email and password (v1 `loginZ`, v3 `loginV2`), auth token |
| `e2ee` | `base/e2ee` | Letter Sealing keys, crypto for text, location and data messages, file crypto |
| `service` | `base/service` | 40 services and 380+ typed methods; any IDL method through `Service::call` |
| `push`, `polling` | `base/push`, `base/polling` | LEGY HTTP/2 push connection; `sync` and `fetchMyEvents` polling |
| `obs` | `base/obs` | media upload and download, plain and E2EE |
| `timeline` | `base/timeline` | posts, likes, comments |
| `rest` | none | Note extras, home profile and cover, URL preview, translation, Keep (404s now). Album writes are in `service::moa`. Reverse engineered from LINE for macOS and not verified |
| `request` | `base/request` | Thrift over HTTP, LEGY encrypted gateway (`gf.line.naver.jp/enc`) |
| `thrift` | `base/thrift`, `@evex/linejs-types` | binary and compact protocols, IDL schema, dynamic `Value` |
| `storage` | `@evex/linejs/storage` | `MemoryStorage`, `FileStorage` (JSON, same format as LINEJS) |

### Calling any method

LINEJS generates about 60k lines of types and writers. line-rs embeds the
IDL (`resources/line.thrift`) instead and parses it once at runtime.
Arguments are encoded as `<method>_args` and replies decoded as
`<method>_result`, so every method in the IDL works without generated code.

```rust
use line_rs::tv;
let profile = base.talk().call("getProfile", tv!({"syncReason": "INTERNAL"})).await?;
println!("{}", profile["displayName"].as_str().unwrap_or(""));
```

Results are `line_rs::Value`: named fields, enums as variant names, and
strings that aren't valid UTF-8 as bytes. Index with `v["field"]`, or
deserialize into your own serde types with `value.deserialize_into::<T>()`.
For methods missing from the IDL, `BaseClient::request` takes and returns raw
wire values (`TValue`).

### Events

`BaseClient::subscribe()` yields `ClientEvent` (`QrCall`, `PinCall`, `Ready`,
`UpdateAuthToken`, `UpdateSyncData`, ...). `Client::subscribe()` yields
`Message`, `MessageEdit`, `Event`, `Talk` (typed read receipts, reactions,
unsends, calls, group membership, contacts), `SquareMessage` and
`SquareEvent`. `Client::listen` uses the push connection; set
`ListenOptions { polling: true, .. }` to poll instead.

## What line-rs adds to LINEJS

Compared with LINEJS at
[`4fc512a`](https://github.com/evex-dev/linejs/commit/4fc512a) (October 2026).

Session handling:

- Can refresh the v3 access token shortly before it expires, using the
  cached expiry (set `Config::proactive_refresh`; it's off by default).
  LINEJS refreshes only after the server answers `MUST_REFRESH_V3_TOKEN`,
  which line-rs also handles.
- Recognizes more logout reasons (`V3_TOKEN_CLIENT_LOGGED_OUT` and others,
  not only `NOT_AUTHORIZED_DEVICE`) and emits `ClientEvent::KickedOut`.
- Fails over to the `legy-backup` host when a connection fails, and can go
  through an HTTP or SOCKS proxy.
- Logs in with a phone number and password.
- The `link` module parses `line://` and `line.me/R/` links.

Messaging:

- `Chat::send_image`, `send_video`, `send_audio`, `send_file` and
  `send_contact` for one-to-one and group chats, encrypted with Letter
  Sealing when the chat uses it. In LINEJS's client only OpenChat has these.
- `ClientEvent::Talk` turns raw operations into typed events: read receipts,
  reactions, unsends, calls, group membership and contacts.
- An encrypted send that the server rejects is retried up to 5 times, with
  the fix the error asks for: re-encrypt with fresh keys, register a new
  group key, or send in plain text when encryption wasn't forced. LINEJS
  retries once with encryption on.
- E2EE media v2 integrity checks (HMAC over 128 KiB chunks) and the E2EE key
  exchange calls.

Reverse engineered from LINE for macOS 26.5, and not yet checked against the
live server:

- Thrift methods that LINEJS's IDL doesn't have: `editMessage`,
  `silentlyUnsendMessage`, `updatePinState`, `getPinnedChats`,
  `getMessagesByIds`, `getUserTicket`, `blockRelation`, `unblockRelation`,
  `getBlockDetails`, `logoutV2`, Premium product tiers and a few more. They
  are appended to the end of `resources/line.thrift`.
- REST calls: more Note calls (announcements, listing and deleting
  comments, reports, search), home profile and cover, URL previews, and
  translation. There is a Keep client too, but LINE's server now answers
  every Keep path with 404.
- Album writes: create, rename, delete and share albums, upload and delete
  photos, like photos. LINEJS can only download album photos.

## Compatibility with LINEJS

- Thrift output is byte for byte the same. The golden vectors in `tests/`
  were generated with LINEJS and node's `thrift` package.
- Storage keys and JSON formats match, so a LINEJS `storage.json` (tokens,
  certificates, E2EE keys, reqseq) can be reused.

## What has been tested

The offline tests cover the Thrift codecs, the crypto and the request
pipeline. Much of the newer API (editing and unsending messages, pins, E2EE
media, the REST endpoints) comes from static reverse engineering of LINE for
macOS and has not been checked against the live server.
[`tests/LIVE_TESTING.md`](tests/LIVE_TESTING.md) lists those guesses and how
to test them on a real account. Reports of what works and what doesn't are
welcome.

## Not ported

- VoIP media (`client/features/call/**` in LINEJS: SRTP, ICE, Opus, Planet)
  and the `@evex/linejs-agent` package.
- The LINEJS docs site and its IDL and APK tooling (`scripts/`).
  `resources/line.thrift` is LINEJS's IDL plus the methods listed above.

## Development

```sh
cargo test                            # offline tests
cargo test --test live -- --ignored   # two calls to the real LINE servers, no login
cargo clippy --all-targets
```

The tests that log in to a real account are in `tests/live_auth.rs`. Read
[`tests/LIVE_TESTING.md`](tests/LIVE_TESTING.md) before running them.

## License

MIT, like LINEJS. See [LICENSE](LICENSE).
