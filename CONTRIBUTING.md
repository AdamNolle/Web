# Contributing to Web

Web is a local-first, finite digest. Contributions must preserve that promise: no infinite feeds,
engagement optimization, telemetry, browser-owned network/filesystem access, scraping, cookie
import, or undocumented-provider fallbacks.

## Set up and verify

Use Node 24+, pnpm 11.3+, and Rust 1.96 with `rustfmt` and `clippy`.

```bash
pnpm install
pnpm verify
pnpm tauri build --ci --no-bundle -- --locked
```

`pnpm verify` is required for every change. The no-bundle build checks the native host target; it
does not prove packaged runtime behavior on Windows, macOS, or Linux. Keep platform claims scoped
to the evidence actually gathered.

## Change boundaries

- Keep the React renderer presentation-only. Network, files, SQLite, credentials, external links,
  and model access belong in narrow, validated Rust commands.
- Treat every feed, archive, HTML fragment, URL, and model response as untrusted input. Preserve
  size bounds, SSRF controls, request receipts, source generations, and privacy invalidation.
- Do not activate a social provider without its current official API, OAuth, policy, retention, and
  deletion evidence. Archive import is not permission to add a live connector.
- Add tests for behavior changes and update `REMAINING-WORK.md` whenever implemented work or
  acceptance evidence changes.

## Pull requests

Explain the user outcome, privacy/security impact, and the exact checks you ran. For visual or
native behavior, include dated host/runtime evidence rather than representing a browser test or
compile as native proof.

Please report vulnerabilities privately using [SECURITY.md](SECURITY.md), not through a public
issue.
