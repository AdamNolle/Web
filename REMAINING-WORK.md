# Web: current state and remaining work

_Last reconciled against the repository and `../web-portal` on 2026-08-08._

This is the canonical product backlog. Older iteration notes in `progress.md`, `.ralph/`, and
`.artifacts/` are useful historical evidence, but they are not an accurate list of what remains.

## Executive summary

Web is no longer just a scaffold. The core RSS-to-finite-edition path is real, local-model and
deterministic-summary paths are real, explicit-feedback ranking and lexical trends are active, the
process-resident scheduler is active, Windows installers have been produced, and the app has been
launched against a real per-user database.

The largest remaining gaps are native acceptance, release hardening, and live social accounts. A
user can build a calm RSS digest, migrate reviewed OPML feeds, search and save collected material,
browse prior editions, use close-to-tray, and restore a local backup in the current working tree.
Those workflows still need packaged/native evidence; no live social account is connected.

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
  - accessible platform-aware command palette: ⌘K on Apple platforms and Ctrl+K on Windows/Linux;
    its Ctrl+K/Escape behavior has release-WebView smoke evidence on Windows, while macOS and Linux
    native proof remains open;
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
   - Completed locally: v15 migrates the durable comment ledger from raw provider identifiers to
     source-scoped SHA-256 fingerprints. The ledger still survives retention to prevent identity
     reassignment, but no longer stores raw comment/post provider IDs; source deletion cascades it.
   - Regression coverage now proves delete-then-reassign, retention-then-reassign, partial/complete,
     transactional rollback for duplicate-A/missing-B prepared sets (cursor/comments/summaries/jobs/
     privacy unchanged), migration, and reversed complete snapshots after reopen.
   - This is an opaque source-scoped fingerprint, not a vault-backed HMAC. Revisit the privacy
     tradeoff only if a future provider requires cross-device durable identity matching.

3. **Refresh visual evidence.**
   - Replace `docs/media/screenshot-today.jpg`; it predates the purple/glass redesign, command
     palette, theme control, and Activity work.
   - On 2026-08-08, the repeatable Windows release-WebView smoke captured and inspected the
     first-run screen at default/light desktop width, dark desktop width, and a compact 800×720
     window. The compact navigation reflows and the dark theme retains the intended hierarchy.
     The screenshot output stays under ignored build tooling rather than being mistaken for a
     published product image.
   - `zoomHotkeysEnabled` is enabled for the primary window, with the narrowly scoped
     `core:webview:allow-set-webview-zoom` capability required by Tauri's macOS/Linux polyfill.
     On 2026-08-08, Windows release-WebView smoke selected the explicit Reading scale control at
     200%, confirmed its selected and persisted values, and captured an inspected enlarged native
     frame. Synthetic Ctrl+Plus still does not expose a measurable WebView2 zoom factor.
   - Still run reduced-motion, loaded, partial, and failure-state visual checks in an actual
     WebView; repeat all visual acceptance, including reading scale, on macOS and Linux before
     release.
   - Confirm a packaged build opens an eligible HTTPS original in the default browser while HTTP,
     credentialed, oversized, and missing URLs remain non-operable.
   - Browser capture was unavailable during this audit, and the checked-in `web-portal/web-app` is
     a macOS ARM64 binary, so no runtime portal comparison is claimed.

4. **Activate and prove CI.**
   - Track/push `.github/workflows/ci.yml`.
   - The latest upstream matrix (2026-07-30) passed Windows and Ubuntu but failed macOS on an
     archive-import test timing race. The assertion now waits for the rendered Activity row; a new
     hosted matrix run is still required.
   - Obtain green Windows, macOS, and Ubuntu runs using the declared Node 24/Rust 1.96 toolchain.
   - Keep the host-native `pnpm tauri build --no-bundle` leg and retain Rust caching/timeouts.
   - Add a packaged smoke lane later; compilation alone does not attest WebView, vault, or migration
     startup.

### P1 — make Web useful every day

1. **Finish the feed-library experience.**
   - Rust-owned OPML picking, per-feed outcomes, bulk review, bounded website-to-explicit-feed
     discovery, and accessible per-row failed-feed edit/retry are implemented. Add packaged native
     acceptance evidence for the file dialog and connection outcomes.

2. **Finish edition history.**
   - Configurable 10–40 item editions, first-pass source fairness, a bounded previous-edition
     list, selectable prior editions, and the concise “since your last edition” summary are
     implemented without a live ticker. Add native visual and assistive-technology evidence.

3. **Refine Search/Recall.**
   - SQLite FTS5, local bounded Library search, retained-text disclosure, and roving keyboard
     navigation are implemented. Add a real-WebView assistive-technology pass.

4. **Refine Saved/read-later.**
   - Explicit Saved lives under Library, survives ordinary retention, and is covered by SQLite
     backup/restore. A native, versioned JSON export now carries the saved content, attribution,
     publication time, safe canonical URL, and retained-summary provenance without database IDs or
     transient source-health details. Add packaged native-dialog evidence for the export path.

5. **Refine model setup.**
   - Installed-model inventory can populate the native picker and all runtime states remain visible.
     Platform-specific Windows/macOS/Linux setup copy now directs users to install/start Ollama
     themselves; Web never downloads a runtime or model.

6. **Source lifecycle controls.**
   - Rename, pause/resume, and bounded per-source sync are implemented for RSS; archive sources are
     manual re-import only. Rename and destructive deletion now use focus-trapped, Escape-dismissable
     in-app dialogs rather than browser-owned prompts. Add native lifecycle acceptance evidence for
     each transition.

7. **Backup/export/restore.**
   - Native SQLite snapshot export/restore validates and migrates a temporary candidate before
     replacement inside an isolated restore workspace, retaining and reopening a rollback snapshot
     on a file-replacement or reopen failure. Export stages an existing backup rather than deleting
     it before publishing a new snapshot. Automated interruption seams cover malformed/future-
     schema candidates, failed export/rename/reopen, stale-artifact cleanup, and successful
     replacement. RSS-only OPML export is implemented through a native save dialog; it round-trips
     escaped titles and public feed URLs without representing archives as subscriptions. Add
     packaged restore/interruption and native-dialog evidence.

8. **Tray/background lifecycle.**
   - Explicit tray Show/Quit actions and a close-to-hide preference are implemented. A packaged
     Windows smoke on 2026-08-08 confirmed that a second launch exits while the first instance stays
     alive and is brought forward. Verify Windows sleep/resume, battery, and clean-exit behavior
     before enabling scheduling by default.
   - Treat OS-level wake services as a separate later feature, not part of the first tray slice.

9. **Item detail and source navigation.**
   - Library items open in an accessible local-detail dialog with focus trapping, Escape/focus
     return, explicit original opening, and source navigation. Bounded Library DTOs now also show
     current source health plus summary method/provider/uncertainty without loading new content.
   - Add narrow-window and real-WebView assistive-technology acceptance evidence.

### P2 — live social sources

1. **Mastodon first.**
   - Completed locally: the current official authorization-server metadata contract is parsed
     fail-closed and can be checked only from an explicit native interaction through the existing
     proxy-free public-address DNS/IP-pinned transport. It reads the fixed same-origin discovery
     endpoint, refuses redirects, caps the response at 64 KiB, and stores nothing. A future
     instance must be root HTTPS and advertise same-origin authorization,
     token, and application-registration endpoints; authorization-code; PKCE `S256`; and broad
     `read` or both `read:accounts` plus `read:statuses`. The preflight performs no registration,
     browser launch, token exchange, or secret persistence.
   - Completed locally: a native desktop orchestration composes preflight,
     dynamic registration, PKCE, a bounded numeric-loopback callback, token exchange, and an
     OS-vault handoff. A person must first run the native compatibility check, then explicitly
     choose the read-only authorization action; it opens the system browser, never renderer
     networking. Before a vault write it persists only an opaque cleanup reference;
     indeterminate vault outcomes seal the request and are deleted on a later launch. Source and
     successful receipt commit atomically, and a newly authorized source remains paused until
     timeline ingestion is implemented. Focused tests cover successful handoff, unknown vault
     finality, and the real local callback.
   - Add provider-acceptance evidence plus source-deletion/locked-vault crash tests on Windows,
     macOS, and Linux before exposing the orchestration.
   - Completed locally: the privileged read-only home-timeline adapter accepts only a Rust-held
     vault token, uses the pinned public-network transport, caps each response at 1 MiB and 40
     statuses, normalizes provider HTML to inert text, and goes through the existing
     generation/cursor-fenced batch persistence and summary preparation path. Boosts and content
     warnings have parser coverage. A source remains paused pending explicit activation evidence.
   - Completed locally: context is fetched only for five root statuses, capped at 256 KiB each,
     and retained as partial evidence with an exact root scope. Unfetched/failed roots are never
     presented as complete; a zero-context batch reports discussion unavailable.
   - Add multi-instance contract tests and provider-acceptance evidence.
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
   - Architectures are explicitly recorded in
     [`docs/release/support-matrix.md`](docs/release/support-matrix.md); its current Windows x64
     local evidence must not be generalized to macOS or Linux.

2. **Supply chain and update path.**
   - Lockfile-enforced builds, dependency/license/secret scans, SBOM, checksums, and provenance.
     The working-tree CI runs `pnpm audit --audit-level=high` and `cargo audit` after locked
     installation. A tested cross-platform `pnpm artifacts:checksums` generator now produces a
     sorted SHA-256 manifest for exact release-candidate artifact paths; release publication and
     provenance remain open.
   - On 2026-08-08, updating Vite/Vitest/ESLint/TypeScript-ESLint reduced npm audit to one low
     advisory. Cargo audit's current remaining report is the Linux-only, Tauri-transitive
     `RUSTSEC-2024-0429` GLib unsoundness plus GTK3 maintenance warnings. It has no confirmed
     remote-to-app path under Web's local-app threat model, but it must be reassessed with the next
     Tauri GTK/WebKit stack update and before Linux release proof.
   - Tauri updater endpoints, signed manifests, rollback behavior, and migration compatibility.
   - The Windows `__TAURI_BUNDLE_TYPE` bundler warning was resolved locally on 2026-08-08 by
     aligning `@tauri-apps/cli` 2.11.4 and `@tauri-apps/api` 2.11.1 with the Rust Tauri 2.11 line;
     fresh MSI and NSIS builds both patched bundle-type information without a warning. Updater
     endpoints and signed manifests still need their own implementation and release evidence.
   - Reproducible release notes that distinguish compile evidence from packaged runtime evidence.

3. **Platform trust.**
   - Windows Authenticode and SmartScreen reputation plan.
   - macOS Developer ID, hardened runtime, notarization, and stapling.
   - Linux checksums/signatures and a finite support matrix.
   - Clean-host install/update/uninstall evidence before calling any platform “released.”

4. **Repository publication and maintenance.**
   - `CONTRIBUTING.md`, focused issue/PR templates, and a monthly grouped Dependabot policy are
     present in the working tree. They become active only after the repository changes are reviewed
     and pushed.
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
| Recall/Saved/Notebook grouping | Use a Library surface; Recall and Saved first                                 | Implemented in current working tree; keyboard/native assistive review remains         |
| “Since last visit” ticker      | Convert to a finite since-last-edition summary                                | Implemented as bounded since-last-edition copy, never a ticker                        |
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

1. Close archive, backup/restore, tray, and source-lifecycle acceptance in packaged native builds.
2. Activate CI and obtain fresh three-host matrix evidence.
3. Add failed-row OPML retry/edit and Library keyboard/assistive refinements.
4. Add item-detail/source-navigation refinements.
5. Build and validate Mastodon.
6. Build and validate Bluesky.
7. Complete signing, updater, supply-chain, and clean-host release gates.
8. Revisit peer sync only after backup/restore and privacy tombstones are proven.

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
