# Web

**Web** is a cross-platform, local-first desktop app that turns selected social and publisher sources into calm, finite editions. It is designed to replace repeated timeline checking—not to maximize engagement.

![Status: foundation MVP](https://img.shields.io/badge/status-foundation_MVP-486b58)

## What works

- Polished browser-preview and Tauri dashboard with Today, Trends, Sources, Activity, and Privacy views.
- Finite editions, stable ordering, explicit feedback/undo, evidence excerpts, and a visible stopping point. Active Not relevant or source-mute feedback suppresses any entire persisted trend whose label or summary used that evidence; Undo/reset can reveal it again without lowering the privacy epoch.
- Rust-owned SQLite database with version-gated transactional migrations, WAL, foreign keys, an empty production first run, payload-bound manual request receipts, retention, feedback/reset, generation-fenced complete source deletion, and non-identifying audit records. Demo fixtures exist only in browser preview. A deleted source has a durable generation tombstone, so a fetch that began before deletion cannot recreate it; deliberate re-add advances the generation. A completed deletion receipt stores only a random replay capability, while the source tombstone owns the durable payload association; same-source replay is idempotent and a different source conflicts without putting a raw identifier or deterministic identifier hash in the receipt/audit row. Add/feedback receipts whose subjects are erased become Unknown rather than false Complete. Upgrades discard validators that predate effective-representation binding, making the next fetch unconditional. Every stale-Unknown mutation, including feedback/settings/reset, fails closed and is not reported as saved or completed.
- Read-only RSS/Atom connector with pinned validated DNS answers, proxy bypass, conservative IPv4/IPv6 special-purpose denial, representation-bound conditional metadata, downgrade rejection, redirect/timeout/response/item budgets, plain-text normalization, optional canonical HTTP(S) links, timestamp provenance, and deterministic fixtures. Validators are bounded and sent only to the effective URL that issued them.
- Provider-neutral Rust connector batches and migrations 9–11 add bounded cursors, typed health/retry/finality state, immutable source-wide comment IDs, scoped complete reconciliation/partial merge, fail-closed legacy repair, and generation/runner/cursor-fenced post/comment persistence. Immutable merged inference candidates bind summaries to exact ordered comment evidence; stale or divergent preparation is rejected. Hard limits are 100 posts, 50 comments per post, 500 comments and 256 KiB of comment text per sync, 4,000 bytes per comment, and depth 8. RSS behavior is unchanged and always reports comments unavailable. Backend descriptors expose Mastodon as validation-required and Bluesky as blocked; the UI has no Connect or credential controls for either.
- Ollama-compatible, numeric-loopback-only provider with exact installed-model metadata, bounded tags/version/structured-generation probes, a five-minute readiness cache, schema-constrained responses, and deterministic per-item fallback. A user may explicitly name an already-installed model; Web never selects, pulls, or downloads one. At most four new or content-changed items per whole sync batch use model generation, serially with strict timeouts; unchanged summaries remain stable. The selected mutable tag is checked against the exact digest immediately before and after each generation, and changed identity discards the output. The source-reading path has no tools or credentials.
- Operating-system credential vault adapter (Windows Credential Manager, macOS Keychain, Linux Secret Service) with no plaintext fallback.
- Typed host/model capability states distinguish unavailable, missing, incompatible, detected-unverified, degraded, and behaviorally ready models. Unknown/unmeasured systems stay on CPU/basic; higher advisory profiles require a bounded local probe plus available-memory headroom. Recommendations never auto-download.
- Rust-owned conditional RSS resync and a single-flight resident runner with renewable owner/fencing-token leases, due-only backoff selection, periodic retention, editable quiet hours, actual eligible schedule state, and at most one catch-up for the nearest missed daily instant. A scheduled instant receives at most one recovery; terminal failure/exhaustion advances the displayed eligible time without pretending success. Every resident source checkpoint, ingest, failure update, and digest publication revalidates the current owner/token/lease in its transaction. A run is capped at 20 sources and eight minutes under a ten-minute renewable lease; each network/model operation is independently bounded below that lease. Resident work has no conflicting manual request receipt. It runs only while Web is open.
- Narrow Tauri commands only; the renderer capability grants no built-in core permissions and has no filesystem, SQL, shell, process, arbitrary HTTP, or external-open capability.

## Honest limitations

Live authenticated social connectors are **not yet shipped**. The Mastodon and Bluesky cards are disabled, backend-driven prerequisite notices only. Official home/following feeds are feasible for Mastodon and Bluesky; YouTube can expose subscription uploads but not its personalized Home feed. Reddit and X require additional approval/cost review. Instagram/Facebook, LinkedIn, and TikTok do not offer the ordinary personal home-feed APIs this product needs. Web does not bypass that gap with stealth, proxy rotation, CAPTCHA bypass, cookies, or undocumented endpoints.

Scheduled sync runs only while the desktop process is open. A bounded tick performs periodic retention and at most one nearest-instant catch-up after startup or wake; there is no hidden OS task, closed-app execution, tray lifecycle, battery/metered integration, or cancellation UI yet. Resident work selects only due sources; the labeled deliberate “Sync all now” action overrides per-source retry timing within the same source/time caps. A timed-out manual request is sealed as an unknown, non-replayable command tombstone because earlier source commits may be retained. The renderer checks calm runner state every 30 seconds, updates source/activity status without harmless reordering, immediately purges privacy-invalidated content, and holds a newly prepared edition until the reader applies it. Manual sync reports complete/partial/unknown finality. Trends in browser preview are labeled fixture data only, and real editions publish no synthetic trends until production clustering is implemented. More/Less feedback is stored locally for future ranking, but learned ranking is not active yet. Canonical source URLs are copyable evidence text; external opening is intentionally not enabled.

## Prerequisites

- Windows 10/11 with WebView2, macOS with WebKit, or a modern Linux distribution with WebKitGTK 4.1
- Node.js 24+ and pnpm 11.3+
- Rust 1.96 with `rustfmt` and `clippy`
- Linux development packages for Tauri/WebKitGTK (see the CI workflow for the Ubuntu list) and an unlocked Secret Service-compatible keyring for authenticated connectors
- Optional: [Ollama](https://ollama.com/) bound to `127.0.0.1:11434`; Web recommends a conservative model profile from local capacity but never downloads or selects cloud models automatically

## Setup and run

```powershell
pnpm install
pnpm dev                 # browser preview with typed demo transport
pnpm tauri dev           # desktop app with the Rust/SQLite core
```

The desktop database is created empty in Tauri's per-user app-data directory. Browser preview is intentionally in-memory demo data and does not exercise credentials or network sync.

## Validation

```powershell
pnpm verify
pnpm tauri build --no-bundle      # compile production desktop binary without installer
pnpm tauri build                   # platform package(s); signing/notarization is required for release
```

Focused commands: `pnpm test`, `pnpm build`, `pnpm rust:test`, `pnpm rust:clippy`.

## Privacy and security defaults

- No remote telemetry or cloud-model fallback.
- Remote media is off by default.
- Tokens live only in the operating-system credential vault; the database stores opaque references.
- Feed/model content is untrusted, length-bounded, stored/rendered as text, and never grants tools.
- RSS blocks localhost, private/link-local/shared/documentation addresses and revalidates DNS and redirects.
- Diagnostic activity stores stable codes/counts, not post text, prompts, secrets, or query-bearing URLs.
- Disconnect-and-delete removes affected posts, comments, summaries, trends, feedback, and digest derivatives; OS backups may retain older copies.
- The SQLite database is not application-encrypted; Web relies on OS account permissions and full-disk protection. OAuth secrets remain separately protected by the OS vault.

See [`docs/security/threat-model.md`](docs/security/threat-model.md), [`docs/product/guardrails.md`](docs/product/guardrails.md), and [`docs/product/connectors.md`](docs/product/connectors.md).

## Structure

- `src/` — presentation-only React app and validated typed transport
- `src-tauri/src/` — trusted commands, DB, connector, model, scheduler, and secret-store boundaries
- `src-tauri/migrations/` — append-only transactional schema
- `tests/fixtures/` — inert deterministic connector fixtures
- `docs/adr/` — durable architecture decisions

## Release requirements

Production releases must add signed updates/installers, macOS signing and notarization, Linux package/repository validation, an SBOM, dependency/license/secret scans, clean-host installer tests on all supported platforms, and dated provider terms approval. The checked-in CI is configured for Windows, Ubuntu, and the native architecture supplied by `macos-latest`, but only Windows has been locally exercised in this repository run. Additional macOS architectures and all non-Windows keyring, runtime, and packaging evidence remain pending native runners. Do not market a connector until its official endpoint, OAuth scopes, quota, attribution, retention/deletion, and derived-data terms are verified.
