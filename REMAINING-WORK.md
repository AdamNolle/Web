# Web: current state and remaining work

_Last reconciled against the repository and `../web-portal` on 2026-07-28._

This is the canonical product backlog. Older iteration notes in `progress.md`, `.ralph/`, and
`.artifacts/` are useful historical evidence, but they are not an accurate list of what remains.

## Executive summary

Web is no longer just a scaffold. The core RSS-to-finite-edition path is real, local-model and
deterministic-summary paths are real, explicit-feedback ranking and lexical trends are active, the
process-resident scheduler is active, Windows installers have been produced, and the app has been
launched against a real per-user database.

The largest remaining gap is product breadth, not foundation quality. A user can build a calm RSS
digest and, in the current working tree, import official X and Instagram archives. A user still
cannot migrate an existing feed library, search or save collected material, browse prior editions,
leave the window while background work continues, restore a backup, or connect a live social
account.

The right direction from `web-portal` is to adapt its best interaction patterns to Web's calmer
finite-edition product. Copying its entire operations cockpit, engagement machinery, or local admin
surface would work against Web's product and security boundaries.

## What is genuinely implemented

### Product

- RSS/Atom sources with bounded conditional synchronization, retention, source health, and explicit
  manual override.
- Finite digest editions with per-source bounds, deterministic fallback summaries, and optional
  schema-validated local Ollama summaries.
- Explicit-feedback-only ranking with minimum-signal gating, a chronological/diversity reserve,
  persisted score components, and why-shown copy.
- Deterministic lexical trends with cross-source gating and same-source duplicate collapse.
- Process-resident scheduling, catch-up, quiet hours, bounded runs, durable leases, and honest
  partial/unknown outcomes. It still stops when the process exits.
- Source deletion, feedback undo/reset, privacy-epoch invalidation, and derivative cleanup.
- Explicit original-link opening through a Rust-owned, credential-free HTTPS-only command. The
  renderer receives no general opener/shell permission, and unsafe or absent URLs stay copyable but
  non-operable.
- In the current working tree: bounded local import of official X `tweets.js`/`tweet.js` and
  Instagram `posts_1.json` exports through a native file picker. Imports are additive, replay-safe,
  never scheduled as live sources, capped at 20 MiB and 25,000 entries, and can be repeated under
  the same archive name. Entries stream one at a time; exact duplicates collapse, conflicting
  identities fail before inference, and Instagram identity uses its normalized media set. Files
  above either bound fail explicitly rather than silently truncating.

### Interface

- Responsive React interface with a finite natural end, visible operation state, semantic focus,
  skip navigation, reduced-motion support, and tested light/dark contrast tokens.
- Intentional zero-source first-run route with clear RSS/archive choices and focus routed to the
  relevant Sources control; the generic caught-up edition remains reserved for connected sources.
- In the current working tree, adapted from the useful parts of `web-portal`:
  - accessible Ctrl/Cmd+K command palette for views, sources, and trends;
  - explicit Auto/Light/Dark presentation modes;
  - Activity-scoped vitals, runner/model state, source-health table, and chronological activity;
  - responsive glass/purple visual system without copying the portal's global live ticker or dense
    six-metric header.

### Engineering and release foundation

- Presentation-only renderer; Rust owns network, SQLite, files, credentials, scheduling, and model
  access.
- SSRF-hardened RSS transport with DNS pinning, redirect revalidation, downgrade rejection, proxy
  bypass, response limits, and normalized canonical links.
- Rust-owned SQLite with WAL, foreign keys, version-gated migrations, source generations, fenced
  resident effects, replay-safe commands, and bounded retention.
- OS-vault abstraction with no plaintext fallback and a restrictive Tauri capability baseline.
- `LICENSE`, `SECURITY.md`, package/Cargo license metadata, real multi-platform icons, stable app
  identifier, and Windows MSI/NSIS configuration.
- Windows MSI and NSIS artifacts have been produced. A real database exists under
  `%APPDATA%\io.github.adamnolle.web`, proving native setup/migrations launched outside unit tests.
- A three-host GitHub Actions workflow exists in the working tree, but it remains untracked and has
  no upstream run evidence yet.

## Claims that were stale

Do not reintroduce these as open tasks:

- Ranking is not a fixed `0.8 - index * 0.04` placeholder anymore.
- Production trends are not inert; digest preparation writes and loads lexical clusters.
- The local-model budget is not still the old unqualified per-item behavior; unchanged items reuse
  summaries and inference is bounded at the whole-run level.
- `LICENSE`, `SECURITY.md`, package license fields, icons, bundle metadata, and the non-placeholder
  application identifier exist.
- Windows bundles have been built and the native app has launched.
- `src/styles.css` is valid. A compressed shell rendering made it look corrupt, but its worktree and
  Git blob were byte-identical before the current design changes.

## Highest-priority remaining work

### P0 — close and prove the current slice

1. **Finish native archive-import acceptance.**
   - Automated coverage now proves the exact file/item bounds and pre-allocation 25,001st-entry
     abort, duplicate collapse/conflict handling, stable Instagram re-import identity, populated
     v12-to-v13 preservation, replay/cancel behavior, same-name additive re-import, and truthful
     partial health; the merged formatter/lint/typecheck/frontend/Rust suite is green.
   - Exercise the native file dialog and both real official export shapes in a packaged Windows
     build.
   - Repeat a named archive through the real UI and confirm the expected update/skip counts and
     absence of duplicate posts.
   - Exercise malformed and exact-duplicate entries through the real UI and confirm the source and
     activity surfaces retain truthful partial health; prove an over-bound file rejects without
     source, receipt, model, or post side effects.
   - Measure a near-limit packaged import. If the bounded operation is still too long for a calm
     foreground action, add progress and post-selection cancellation before release.
   - Add a repeatable native-dialog end-to-end test or retain equivalent dated manual evidence.

2. **Close the round-12 social-foundation acceptance gaps.**
   - Add delete-then-reassign, retention-then-reassign, partial/complete, rollback, reopen, and
     migration-12 tests for the durable comment identity ledger.
   - Add the duplicate-A/missing-B prepared-set regression and assert no cursor, comment, summary,
     job, or privacy side effect.
   - Add reversed-complete-batch coverage proving stable model input and stable identity after
     reopen.
   - Review whether plaintext `remote_id` and `post_remote_id` in the durable ledger are acceptable
     under the retention promise. Prefer keyed, source-scoped fingerprints if raw provider
     identifiers are not required for diagnostics.

3. **Refresh visual evidence.**
   - Replace `docs/media/screenshot-today.jpg`; it predates the purple/glass redesign, command
     palette, theme control, and Activity work.
   - Run keyboard, narrow-window, 200% zoom, reduced-motion, light, dark, empty, loading, partial,
     and failure-state visual checks in an actual WebView.
   - Confirm a packaged build opens an eligible HTTPS original in the default browser while HTTP,
     credentialed, oversized, and missing URLs remain non-operable.
   - Browser capture was unavailable during this audit, and the checked-in `web-portal/web-app` is
     a macOS ARM64 binary, so no runtime portal comparison is claimed.

4. **Activate and prove CI.**
   - Track/push `.github/workflows/ci.yml`.
   - Obtain green Windows, macOS, and Ubuntu runs using the declared Node 24/Rust 1.96 toolchain.
   - Keep the host-native `pnpm tauri build --no-bundle` leg and retain Rust caching/timeouts.
   - Add a packaged smoke lane later; compilation alone does not attest WebView, vault, or migration
     startup.

### P1 — make Web useful every day

1. **Feed discovery and OPML.**
   - Accept OPML through a Rust-owned native picker and show per-feed success/failure.
   - Resolve a normal website URL to advertised RSS/Atom feeds through the hardened Rust network
     boundary; do not fetch from React.
   - Support bulk review before adding dozens of feeds.

2. **Configurable, fair editions and history.**
   - Make edition size configurable within a calm finite range such as 10–40.
   - Guarantee one eligible item per source before a source receives a second slot.
   - Expose previous editions from existing digest rows, with clear generated-at and source-change
     context.
   - Add a concise “since your last edition” summary instead of a perpetual live ticker.

3. **Search/Recall.**
   - Add SQLite FTS5 over retained title/body/author/source text, with migration and rebuild tests.
   - Make a Library surface for Recall results and later Saved items.
   - Keep search local, bounded, keyboard accessible, and explicit about retention limits.

4. **Saved/read-later.**
   - Add an explicit saved state, not a passive engagement signal.
   - Define whether saved items survive ordinary retention; if they do, make that exception visible
     and exportable.
   - Add Saved under Library rather than another global dashboard tab.

5. **Installed-model picker and model setup.**
   - Surface the already queried Ollama model inventory as a picker instead of a free-text-only
     field.
   - Explain runtime unavailable, model missing, incompatible, unverified, and degraded states with
     direct remediation.
   - Keep downloads user-initiated; never silently install a runtime or model.

6. **Source lifecycle controls.**
   - Add pause/resume, rename, and a bounded “sync/retry this source” action without requiring
     destructive disconnect/re-add.
   - Keep archive sources manual-only and route their equivalent action to re-import.
   - Preserve source generations, retry eligibility, request receipts, and truthful per-source
     health through every transition.

7. **Backup/export/restore.**
   - Export OPML, settings, feedback/saved state, and a versioned JSON or SQLite backup through
     Rust-owned native dialogs.
   - Use SQLite's online backup API or a consistent snapshot; never copy a live WAL database
     naively.
   - Validate restore into a temporary database, migrate it, then replace atomically with a
     recoverable rollback path.

8. **Tray/background lifecycle.**
   - Add an explicit tray icon, close-to-hide preference, and true Quit action.
   - Keep the existing Rust scheduler/lease machinery; change only process lifecycle.
   - Verify Windows sleep/resume, duplicate-instance, battery, and clean-exit behavior before
     enabling scheduling by default.
   - Treat OS-level wake services as a separate later feature, not part of the first tray slice.

9. **Item detail and source navigation.**
   - Evolve the current inline evidence expansion into an accessible detail drawer when the added
     context justifies it.
   - Show provenance, summary method/uncertainty, source health, and related items without hiding
     the canonical evidence.
   - Preserve focus return, Escape handling, narrow-window reflow, and reduced motion.

### P2 — live social sources

1. **Mastodon first.**
   - Dynamically register a read-only client per instance.
   - Use PKCE, native browser authorization, bounded callback/session state, and the existing OS
     vault.
   - Ingest the home timeline and bounded status context with exact complete/partial provenance.
   - Sanitize provider HTML, enforce source generation/cursor fencing, and add multi-instance
     contract tests.
   - Revalidate instance policy, scopes, attribution, retention, deletion, and rate limits at
     implementation time.

2. **Bluesky second.**
   - Reuse the native browser/vault/session foundation from Mastodon.
   - Publish stable HTTPS client metadata and own the callback origin.
   - Implement PAR, DPoP, nonce rotation, permission validation, moderation parity, and tests
     against entryway plus independent PDS behavior.
   - Keep the descriptor blocked until all activation evidence exists.

3. **Do not activate Reddit, X API, Meta, LinkedIn, or TikTok by implication.**
   - Archive import is not a live connector.
   - Each future provider needs an explicit dated access/cost/policy/retention decision.
   - No cookie import, session replay, scraping fallback, identity rotation, or anti-bot evasion.

### P3 — distributable releases

1. **Native evidence matrix.**
   - Packaged launch, migration, WebView hostile-content, vault round trip, offline mode, CPU-only
     model fallback, sleep/resume, update, and uninstall checks on Windows.
   - Equivalent native package/runtime/keyring evidence on supported macOS and Ubuntu targets.
   - Decide and document architectures explicitly rather than implying universal support.

2. **Supply chain and update path.**
   - Lockfile-enforced builds, dependency/license/secret scans, SBOM, checksums, and provenance.
   - Tauri updater endpoints, signed manifests, rollback behavior, and migration compatibility.
   - Resolve the current Windows bundler warning that `__TAURI_BUNDLE_TYPE` was not found while
     patching the binary; align/verify Tauri CLI and Rust crate behavior before updater work.
   - Reproducible release notes that distinguish compile evidence from packaged runtime evidence.

3. **Platform trust.**
   - Windows Authenticode and SmartScreen reputation plan.
   - macOS Developer ID, hardened runtime, notarization, and stapling.
   - Linux checksums/signatures and a finite support matrix.
   - Clean-host install/update/uninstall evidence before calling any platform “released.”

4. **Repository publication and maintenance.**
   - Add `CONTRIBUTING.md` with the exact `pnpm verify` and native build expectations.
   - Add focused issue/PR templates and a dependency-update policy; enable Dependabot or Renovate
     only with lockfile-preserving grouped updates and CI.
   - Add frontend integration/native E2E coverage before using a coverage percentage as a gate.
   - Decide whether a contributor code of conduct and support policy are appropriate before public
     issue intake.

## `web-portal` parity decisions

| Portal pattern                 | Decision for Web                                                              | Status / next step                                                                    |
| ------------------------------ | ----------------------------------------------------------------------------- | ------------------------------------------------------------------------------------- |
| Command palette                | Adapt as an accessible dialog/combobox                                        | Implemented in current working tree                                                   |
| Auto/Light/Dark theme          | Adapt while retaining Web's purple identity                                   | Implemented in current working tree                                                   |
| Global six-metric strip        | Scope useful metrics to Activity                                              | Implemented as Activity vitals                                                        |
| Run timeline and source health | Adapt with truthful bounded states and table semantics                        | Source health and chronological activity baseline implemented; timeline remains later |
| Detail drawer                  | Adapt for item evidence, provenance, source, and related items                | P1                                                                                    |
| Recall/Saved/Notebook grouping | Use a Library surface; Recall and Saved first                                 | P1                                                                                    |
| “Since last visit” ticker      | Convert to a finite since-last-edition summary                                | P1                                                                                    |
| Git peer sync                  | Redesign around Rust HTTPS Git, vault secrets, conflict rules, and tombstones | Later, after backup/restore                                                           |
| Environment/model diagnostics  | Keep in Activity/Settings, not a global identity bar                          | Baseline implemented                                                                  |
| Dense charts/heatmaps          | Only with responsive reflow, text/table alternatives, and reduced motion      | Selectively later                                                                     |
| Intelligence/Studio/Chat       | Keep out of the primary product until backend states are real                 | Deferred/labs only                                                                    |

### Deliberately not porting

- The entire 16-tab operations cockpit.
- A perpetual global ticker or globally sticky six-metric band.
- Agent society/chat, mastermind theater, raw widget builders, or local script execution.
- Engagement, virality, reward, streak, urgency, or passive-behavior optimization.
- Public unauthenticated admin APIs, wildcard CORS/WebSocket access, or a browser-exposed local
  control plane.
- Portal markup's mouse-first, color-only, fixed-width, and inline-style accessibility debt.
- Semantic/embedding features that are labels or placeholders rather than verified local behavior.

## Cross-device sync, later

`web-portal` demonstrates a useful serverless shape: each installation writes its own append-only
records under an instance directory in a shared Git repository and imports peer records. Web should
not copy that implementation directly.

Before implementation, define:

- which entities sync (sources without credentials, settings, explicit feedback, saved items,
  edition metadata) and which stay local (vault handles, runner leases, jobs, model cache, transient
  health);
- stable per-install instance IDs and monotonic cursors;
- source deletion, feedback reset, retention, and saved-item tombstones so imports cannot resurrect
  private data;
- deterministic merge/conflict rules and schema-version negotiation;
- repository size/compaction limits and recovery from force-push, partial clone, and offline edits;
- a Rust-native HTTPS Git client or narrowly constrained helper with vault-backed credentials;
- clear privacy copy: Git hosting is remote storage even if no Web-operated server exists.

Backup/restore must ship before sync. It provides the serialization, migration, and recovery
primitives needed to make sync safe.

## Recommended execution order

1. Finish and fully verify archive import plus the portal-inspired frontend slice.
2. Close the round-12 regression/privacy evidence and activate CI.
3. Add feed discovery and OPML import.
4. Add pause/resume, rename, and bounded per-source sync/retry.
5. Add fair configurable editions and previous-edition history.
6. Add local Search/Recall, then Saved and the Library surface.
7. Add the installed-model picker.
8. Add item-detail/source-navigation refinements.
9. Add backup/export/restore.
10. Add tray/close-to-hide and native lifecycle evidence.
11. Build and validate Mastodon.
12. Build and validate Bluesky.
13. Complete signing, updater, supply-chain, and clean-host release gates.
14. Revisit peer sync only after backup/restore and privacy tombstones are proven.

## Definition of done

A feature is not done because a type, migration, or screen exists. It is done when:

- the real native route is reachable from the UI;
- renderer input and Rust DTOs reject malformed/unknown fields;
- replay, cancellation, partial failure, privacy deletion, reopen, and migration behavior are
  tested where applicable;
- user-visible health and finality match persisted truth;
- keyboard, focus, zoom, contrast, empty/loading/error, and narrow-layout states are covered;
- `pnpm verify` passes from a clean checkout;
- the relevant host-native build or packaged smoke test passes;
- documentation describes exactly the behavior that shipped, including what remains unavailable.
