# Security

line-rs handles LINE auth tokens and Letter Sealing (E2EE) keys, so a bug
here can expose someone's messages or account.

## Reporting a vulnerability

Report it privately through GitHub: open the **Security** tab of this
repository and choose **Report a vulnerability**. Please don't open a public
issue or pull request for it.

Useful things to include:

- what an attacker can do, and what they need first
- the affected code (file and function) and the commit you tested
- a way to reproduce it, ideally without a real account

Never send real auth tokens, refresh tokens, E2EE keys or `storage.json`
contents, even privately. Use made-up values, or revoke the session first.

## What counts

- Mistakes in the E2EE code: wrong key handling, a weak or predictable
  secret, a message or file sent without the encryption the caller asked for
- Tokens or keys ending up in logs, errors or places other than the storage
  the caller provided
- A malicious server response that crashes the client or makes it act on
  the attacker's behalf

LINE's own servers and apps are out of scope. Report those to LINE.

## Supported versions

Only the latest commit on `main` gets fixes.
