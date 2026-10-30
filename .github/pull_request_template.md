## What changed

<!-- What the change does and why. Link the issue if there is one. -->

## Where the protocol details come from

<!--
For new or changed LINE calls, fields or crypto. Delete this section otherwise.
- Ported from LINEJS: link the file or commit.
- Reverse engineered: which LINE app, version and platform, and what you read.
- Observed on the wire: what you captured (with tokens and IDs removed).
-->

## How I tested it

<!-- Delete the lines that don't apply. -->

- [ ] `cargo test`
- [ ] `cargo clippy --all-targets` has no warnings
- [ ] Against the live server with a real account: which tests in `tests/live_auth.rs`, or what you ran by hand, and what happened
- [ ] Not tested against the live server (say so, so the README can mark it unverified)

## Checklist

- [ ] No auth tokens, refresh tokens, E2EE keys, `storage.json` contents or real LINE IDs (mids) in code, tests, logs or this PR
- [ ] Golden vectors and fixtures use made-up data
- [ ] `README.md` updated if this adds or removes something listed there (modules, "What line-rs adds to LINEJS", "Not ported")
