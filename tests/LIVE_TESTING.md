# Live testing (`tests/live_auth.rs`)

Smoke tests for the authenticated API against the real LINE server. They
are `#[ignore]`d, refuse to run without `LINE_TEST_ACCOUNT_CONFIRM=yes`, and
are never run in CI.

## Read this first: these tests use a real account

> **These tests log in to a real LINE account.** If LINE flags the session
> and bans or restricts the account, you can lose its chats, friends,
> groups, stickers and purchases. Use a spare account if you have one.

- Unofficial clients violate LINE's Terms of Service. The measures below
  (one login per run, at least 5 s between requests, no retries, a handful
  of requests per test, writes only to your own Keep Memo chat) reduce load
  and risk. They **cannot** guarantee you will not be banned.
- Run `read_only` first and on its own. Only move on if you accept the risk.
- Keep volume tiny: one module per day or so, stop at the first failure, do
  not loop the tests.
- Nothing is sent to other people by default. Tests that target another
  user (`block_roundtrip`, peer mute/favorite, read receipts, peer block
  details) only run with `LINE_PEER_MID=u...` **and**
  `LINE_ALLOW_PEER_TESTS=yes`. Leave those unset unless you have a willing
  friend who knows what is going on.

## Device type and your LINE desktop app

LINE allows one session per secondary device type. A `DESKTOPWIN` or
`DESKTOPMAC` session from these tests **will likely log out LINE for Mac /
Windows** if you use it, and logging back in there will in turn kill the
test session.

- The token is bound to the device it was issued for. `cargo run --example
  ping` logs in as `Device::DesktopWin`; the tests default to
  `LINE_TEST_DEVICE=DESKTOPWIN` to match.
- If you use LINE desktop on your Mac, expect it to be signed out while you
  test, or obtain the token with a device type you do not otherwise use
  (edit the `Device::DesktopWin` line in `examples/ping.rs` locally, e.g. to
  `Device::IosIpad` if you have no iPad, and set `LINE_TEST_DEVICE=IOSIPAD`).
  Untested combinations may be refused by the server.

## Setup

1. Get a token once with a QR login (scan with your phone):

   ```sh
   cargo run --example ping   # no env vars -> QR login
   ```

   Copy the `auth token (reuse with LINE_AUTH_TOKEN): ...` line, then stop
   it with Ctrl-C. Do not log in repeatedly; reuse that token.
2. The tests keep state (reqseq, E2EE keys, refresh token) in
   `./storage.test.json` (git-ignored). Keep it between runs. If the token
   is rotated during a run the new one is printed as `NOTE: auth token
   rotated`; put it in `LINE_AUTH_TOKEN` for the next run.
3. Optional inputs:

   | Variable | Used by |
   |---|---|
   | `LINE_GROUP_MID` | a group you are in: `read_only::group_*`, `write::group_mute_favorite_roundtrip` (local settings only, members see nothing) |
   | `LINE_KEEP_CONTENT_ID` | `read_only::keep_get_content` |
   | `LINE_PRODUCT_REGION` | `read_only::product_tiers_for_client` (default `TH`) |
   | `LINE_PEER_MID` + `LINE_ALLOW_PEER_TESTS=yes` | tests aimed at another user (off by default) |
   | `LINE_ALLOW_KICKOUT_TEST=yes` | `session::kicked_out` (off by default, ends the session) |

   A missing optional input prints `skipped: <reason>` and the test passes.

## Commands

Always pass `--ignored --test-threads=1 --nocapture`. Tests share one client
and one throttle; parallel runs would defeat the throttle.

```sh
export LINE_TEST_ACCOUNT_CONFIRM=yes LINE_AUTH_TOKEN='...'

cargo test --test live_auth read_only -- --ignored --test-threads=1 --nocapture
cargo test --test live_auth write     -- --ignored --test-threads=1 --nocapture
cargo test --test live_auth events    -- --ignored --test-threads=1 --nocapture
cargo test --test live_auth e2ee      -- --ignored --test-threads=1 --nocapture
cargo test --test live_auth session::proactive_refresh -- --ignored --test-threads=1 --nocapture
```

Single test: `cargo test --test live_auth write::edit_message -- --ignored --test-threads=1 --nocapture`.

## UI (`cargo run --example test_ui`)

A local web dashboard over the same test cases (copied into
`examples/test_ui/cases.rs`; this file's tests are unchanged).

```sh
cargo run --example test_ui              # TEST_UI_PORT=7878 by default
# open the printed http://127.0.0.1:7878/?k=<secret> URL
```

- Binds 127.0.0.1 only. Every request needs the per-run secret (`?k=` or
  `x-test-ui-key` header) and a `127.0.0.1`/`localhost` Host header, else 403.
- Tick the risk checkbox first; login and runs are refused without it.
- Login: QR (rendered in the page, PIN shown), pasted token, or the token
  saved from a previous UI login (`testUi:authToken` in
  `./storage.test.json`, kept server side; the page only sees mid + device).
- Same gating as here: peer tests need a peer mid **and** the peer toggle,
  `session::kicked_out` needs its own toggle. Throttle is at least 5 s, one
  test at a time, no retries. Interactive tests show a "ทำสิ่งนี้บนมือถือ"
  banner with a countdown and a cancel button.
- "เลือก…" next to the group/peer mid inputs opens a searchable list of
  joined groups (`getAllChatMids` + `getChats`, 2 calls) or friends
  (`getUserFriendIds` + `getContactsV3` per 100 friends, falling back to
  `getContactsV2` where V3 is "API method not capable", e.g. desktop). Loaded on first
  open, cached until "รีเฟรช" or logout, every call waits on the throttle.
  Picking a peer does not enable the peer toggle. The Keep picker and the
  `keep_*` tests make no requests: LEGY does not route `/kp` (live run
  2026-10-21: every `/kp` path, real or not, got a bodiless 404 from
  `server=legy`).
- Live event panel (SSE) shows every `ClientEvent` once logged in; the talk
  listener starts right after login.
- Write/interactive tests take a manual verdict and notes; "Export
  results.md" downloads the results table. "Stop all" clears the queue,
  aborts the running test and drops the client.

## Recommended order

1. `read_only` (no state changes).
2. `write` (self-chat only by default).
3. `events` (you act on your phone, same account).
4. `e2ee` (you send a photo from your phone).
5. `session::proactive_refresh`.

One module per day or so. Stop at the first failure and record it below
before going on.

`session::kicked_out` is opt-in. It waits 180 s for you to log the test
session out from the phone (Settings > Account > Devices). The token is dead
afterwards and you need a new QR login, which is one more login event on
your account. Skip it unless you specifically need to verify kick-out
handling.

## What to check on your phone

All writes land in **Keep Memo** (your self-chat) unless noted.

| Test | Expect on phone |
|---|---|
| `write::send_text` | a `[line-rs live test] send_text <ts>` text |
| `write::send_image` | a 64x64 solid green image, opens normally |
| `write::send_file` | file `line-rs-live-test.txt`, downloads, contains the test text |
| `write::send_contact_of_self` | a contact card for yourself with your name |
| `write::edit_message` | the "(before)" text replaced by "(after)", marked edited (may need LINE Premium) |
| `write::silently_unsend_message` | the message disappears with no "unsent" notice (may need Premium) |
| `write::pin_roundtrip` | Keep Memo pinned then unpinned in the chat list (end state: unpinned) |
| `write::react_roundtrip` | a reaction appears then goes away (server may refuse reacting to your own message) |
| `write::group_mute_favorite_roundtrip` | group notifications and favorite back to their original state (end state: unmuted, not favorite) |
| `write::peer_mute_favorite_roundtrip` | (opt-in) contact settings back to unmuted, not favorite |
| `write::block_roundtrip` | (opt-in) peer not blocked at the end; if unblock failed, unblock manually |
| `events::reaction` | you react to the test text in Keep Memo from the phone |
| `events::unsend` | you send a text in Keep Memo and unsend it |
| `events::read_receipt` | (opt-in) peer opens the chat |
| `e2ee::incoming_image_download` | you send a photo to Keep Memo |

Note: if the tests restore something to "off" that you had "on" before
(e.g. a group was already a favorite), set it back by hand.

## Unverified guesses being tested

Much of the newer API comes from static reverse engineering of LINE for macOS and has never
been checked against the live server. A failure most likely means one of
these guesses is wrong:

| Guess | Exercised by |
|---|---|
| Thrift arg/field ids of reverse-engineered methods (`editMessage`, `silentlyUnsendMessage`, `updatePinState`, `getPinnedChats`, `getMessagesByIds`, `getUserTicket`, `blockRelation`/`unblockRelation`, `getBlockDetails`, `getProfiles`, `getProductTiersForClient`) | the matching `read_only` / `write` test |
| Message `contentMetadata` keys written from memory (contact card, file name/size, image) | `send_contact_of_self`, `send_file`, `send_image` |
| E2EE v2 chunked media layout / HMAC | `send_image`, `send_file`, `e2ee::incoming_image_download` |
| REST channel ids (Keep, Note/Home) and paths | `keep_size`, `keep_get_content`, `page_info` |
| Translation endpoint/body, `translatedText` response key | `translate_hello_to_thai` |
| Talk event param layouts (`param1..3` of read, reaction, unsend) | `events::*` (raw ops are printed while waiting) |
| Own-device actions arriving as `NOTIFIED_*` ops | `events::reaction`, `events::unsend` in Keep Memo |
| Proactive V3 token refresh | `session::proactive_refresh` |
| `getRSAKeyInfo(3)` phone login, meeting id used as `urlId` | not covered here (would need extra logins/calls) |

## Results template

Copy per run:

```
Date:            Commit:            Device:
| test | pass/fail/skip | notes (error text, what the phone showed) |
|---|---|---|
| read_only::pinned_chats | | |
| read_only::profiles | | |
| read_only::block_details_for_peer | | |
| read_only::product_tiers_for_client | | |
| read_only::user_ticket | | |
| read_only::messages_by_ids | | |
| read_only::keep_size | | |
| read_only::keep_get_content | | |
| read_only::page_info | | |
| read_only::translate_hello_to_thai | | |
| read_only::group_announcements | | |
| read_only::group_read_range | | |
| write::send_text | | |
| write::send_image | | |
| write::send_file | | |
| write::send_contact_of_self | | |
| write::edit_message | | |
| write::silently_unsend_message | | |
| write::pin_roundtrip | | |
| write::react_roundtrip | | |
| write::group_mute_favorite_roundtrip | | |
| write::peer_mute_favorite_roundtrip | | |
| write::block_roundtrip | | |
| events::read_receipt | | |
| events::reaction | | |
| events::unsend | | |
| e2ee::incoming_image_download | | |
| session::proactive_refresh | | |
| session::kicked_out | | |
```
