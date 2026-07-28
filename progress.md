# Web implementation progress

Updated: 2026-07-27 — Ralph iteration 5

## Completed foundation

- Cross-platform Tauri 2 + React + TypeScript + Rust scaffold with a presentation-only renderer and restrictive CSP/capability baseline.
- Rust-owned SQLite, RSS connector, OS-vault abstraction, loopback model boundary, deterministic fallback, finite dashboard, documentation, tests, icons, and native-host CI configuration.
- Windows `pnpm verify` and `pnpm tauri build --no-bundle` pass; macOS/Linux native evidence remains pending.

## Iteration 2 hardening completed

### RSS/IPC security

- RSS rejects mapped/compatible IPv6, private, loopback, link-local, shared, benchmark, documentation, multicast, reserved, and selected special-purpose destinations.
- Domain requests reject any mixed unsafe DNS answer and pin the exact validated address set into reqwest while preserving the hostname for TLS. Proxies and automatic redirects are disabled; every redirect is independently re-resolved/re-pinned; HTTPS downgrade is rejected.
- Request/URL/header/component bounds and `deny_unknown_fields` validation run before network access.
- Durable payload-bound request receipts prevent completed add-source/settings/delete/reset request IDs from replaying effects; pending requests fail closed.
- Ollama health/chat bodies are stream-limited before whole-response allocation.

### Persistence/privacy correctness

- Migrations are version-gated and transactional through schema versions 1 and 2.
- Demo fixture seeding has a durable one-time marker; a deleted fixture source does not reappear after reopening the database.
- Real digest generation no longer creates hard-coded demo trends and uses a deterministic two-per-source baseline.
- RSS updates replace stale summaries, and dashboard joins select exactly one current content-addressed summary with provider/method/uncertainty provenance.
- Retention is enforced against local fetch time at startup/dashboard and settings updates; affected digest/trend derivatives are removed transactionally.
- Source deletion removes affected digest/trend-derived text and writes only a non-identifying audit outcome. Delete/reopen and post-delete digest tests pass.
- Reset learning is implemented in Rust, Tauri transport, and demo transport; source mute is exposed with undo.
- Activity `queued` state and Rust MSRV contract drift were corrected; vault secrets use a conservative 2,048-byte portable ceiling.

### Calm UX/accessibility correctness

- Semantic light/dark color tokens replace failing hard-coded pairs for text, inputs, controls, danger actions, and focus indicators.
- Feedback success/undo is shown only after persistence; controls are single-flight and failure is tested.
- Summary method/provider and uncertainty are visibly labeled; evidence includes time and accurately offers a source URL rather than a non-operable “original” claim.
- Initial load has retry, safe backend messages are preserved, failed RSS additions keep form input, invalid settings cannot save, route headings receive focus, and Trends/Activity have empty states.
- SQLite encryption/backup disclosure is accurate and CSP now includes `frame-ancestors 'none'`.
- Scheduling UI and README now state that the runner/catch-up/tray lifecycle is pending instead of claiming execution.

## Iteration 3 foundation integrity completed

### Network and source normalization

- RSS now uses a conservative `2000::/3` global-unicast admission rule plus explicit denials for special space within it; NAT64, local translation, discard/dummy, 6to4, mapped/compatible, private, and reserved cases are tested.
- Ollama bypasses system proxies and redirects. A listed model is only `detected_unverified`; `ready` requires a bounded structured-generation probe.
- RSS parsing uses the effective feed URL as its base, stores only optional canonical HTTP(S) links, rejects opaque/unsafe schemes as source links, and preserves `published`, `updated`, or `fetched` timestamp provenance through Rust/TypeScript/UI.
- Completed source deletion removes its receipt in the same SQLite transaction, and migration 3 purges legacy deletion receipts.

### Production database semantics

- Migration 3 removes legacy desktop fixtures and production startup now creates a valid empty edition/settings state only. Fresh, reopen, and v1-upgrade tests contain no fake sources, posts, trends, or activity.
- Feedback is payload-bound and replay-safe; undo is idempotent; reset keeps feedback receipts so delayed pre-reset requests cannot restore deleted feedback.
- Settings persistence and retention are one transaction; old comments expire independently; future schema versions fail closed; dashboard items require an exact current content-hash summary.
- Browser-preview trend fixtures prune hidden/deleted evidence and are explicitly labeled demonstrations.

### Typed capability and calm UX integrity

- Shared typed model states distinguish checking, unknown, runtime unavailable, model missing, incompatible, detected-unverified, degraded, and behaviorally ready. Host capabilities are typed, expose available memory, and never elevate unknown/unmeasured systems above CPU/basic.
- Sources and editions expose no speculative runner time. UI copy states that learned ranking, clustering, recurring sync, and scheduling are not active.
- Semantic dark-mode hover tokens replace the remaining failing colors. Settings errors are field-associated/announced without empty-string number coercion, route focus changes only on navigation, fixture labels are accurate, and Sources has an empty state.
- The macOS support claim is narrowed to the native architecture exercised by `macos-latest`; additional architectures remain unattested. Vault secret byte-boundary tests cover ASCII and multibyte UTF-8.

## Iteration 4 activation and acceptance work completed

- Migration 3 now defaults unknown legacy timestamps to `fetched`; migration 4 reruns Rust URL sanitation, nulls malformed/credentialed/fragmented/unsafe legacy values, resets old provenance conservatively, zeroes non-authoritative digest placeholders, and adds authoritative runner/receipt-source state. Direct v2 upgrades and rollback/future-version behavior are tested.
- Light input boundaries and dark fixed-surface focus rings meet 3:1 in automated token checks. Undo/Dismiss restore a stable feedback origin or heading, Settings no longer remounts after save/reset, evidence labels are item-specific, URL text is honestly non-operable, and More/Less is labeled future-ranking feedback.
- Source deletion converts linked add/feedback/delete hashes to command-only replay tombstones. Stale pending requests fail closed after 15 minutes, DemoTransport mirrors feedback replay/reset, and OS-vault reads revalidate the portable byte ceiling.
- Users may name one already-installed Ollama model; blank selects deterministic extraction. Exact identity/digest/size/parameter/quantization/runtime metadata is carried from the runtime, readiness requires a structured behavioral probe, and host envelopes no longer infer model size. At most four new items per whole sync use serial timed local generation; exact provenance is persisted and every item can fall back.
- `sync_sources` uses persisted ETag/Last-Modified, 304 checkpoints, per-source backoff/health, atomic ingest/checkpoints, and finite edition preparation. A Rust-owned 60-second resident tick runs retention, uses a durable expiring single-flight lease, respects schedule/quiet hours, and catches up only the nearest missed instant while Web is open. The UI exposes runner/source timing and partial failures.

Round-3 artifacts remain `.artifacts/reviews/round3-{security,correctness,ux,portability}.md`.

## Round-4 acceptance findings queued for iteration 5

- Block source resurrection by serializing deletion with sync and enforcing a durable source generation/tombstone in the ingest transaction.
- Redesign runner job/receipt recovery as one owner-fenced state machine with renewal/heartbeat, compare-and-set completion, and restart tests around lease/receipt boundaries.
- Associate validators with the effective feed URL, rotate validators from 304 responses, enforce `next_poll_at`/backoff for resident work, and make manual override explicit.
- Budget model generation only for new/content-changed posts and preserve unchanged summaries.
- Re-attest immutable model identity around generation so persisted digest provenance cannot describe a replaced mutable tag.
- Push/poll calm runner state into the open renderer, calculate actual quiet-hour-adjusted next execution, and announce partial manual sync accurately.
- Bound incoming response validators, make vault deletion idempotent on missing credentials, narrow `core:default`, and improve runner/source labels and feedback consequence copy.

Round-4 artifacts: `.artifacts/reviews/round4-{security,correctness,ux,portability}.md`.

## Iteration 5 concurrency, identity, and live-runner hardening completed

- Migration 5 adds source generations, durable deletion tombstones, validator representation URLs, and runner owner/fencing state. Existing-source ingest and 304/failure checkpoints require the exact fetched generation; only deliberate add can insert/re-add, and re-add advances the generation. Delete now shares the in-process sync gate. A deterministic paused-fetch/delete/resume test proves sources, posts, receipt bindings, and identifying receipt hashes do not return.
- Validators are bounded and syntax-checked on receipt/use, persisted with the effective representation URL, sent only to that URL, rotated from valid 304 responses, and not forwarded across redirects. Resident selection enforces `next_poll_at`; typed manual override is explicit. Runs cap at 20 sources/eight minutes.
- Posts are content-hash classified before inference. Unchanged summaries/provenance remain stable; only new/content-changed posts enter the four-attempt whole-batch model budget. Ollama generation now re-attests the exact digest immediately before and after every attempt and discards output if the mutable tag changes.
- Scheduled work no longer creates a manual request receipt. Acquisition returns a unique owner and monotonic fencing token, heartbeat renews both runner/job leases, and compare-and-set finish rejects stale owners. Expired outcomes become recoverable `unknown`; complete, partial, failed, and unknown states remain distinct. DB tests inject explicit wall-clock values for contention, heartbeat, stale A after B, and recovery windows; they are not Tokio paused-time orchestration tests.
- Vault deletion treats an already-missing entry as success. The renderer capability grants no built-in core defaults. Capability and vault mapping tests enforce both contracts.
- The renderer performs a calm bounded local state check, updates runner/source state, and holds a new edition for deliberate application. Manual sync returns typed changed/unchanged/failed counts with partial-result routes. Quiet hours are editable; conflicting schedule hours are rejected; next execution is quiet-hour aware; real IANA timezone DST gap/repeat tests are included. Source/evidence/feedback labels and light/dark contrast coverage were corrected.

## Round-5 acceptance findings queued for iteration 6

- Clear legacy validators on v4→v5 when no proven representation URL exists.
- Immediately remove retention/deletion-invalidated items from any pending/open renderer snapshot while deferring only harmless additions/reordering.
- Preserve a fail-closed replay tombstone and typed partial outcome when manual timeout follows committed source effects.
- Limit unknown scheduled-run recovery, and calculate next eligible time from handled terminal instants rather than last success alone.
- Fence every resident source-side effect with current runner owner/token and make runtime heartbeat/deadline behavior testable under suspend/clock divergence.
- Keep pending editions stable across unrelated mutations, render visible operation errors/progress, and fix secondary-control contrast/focus gaps.

Round-5 artifacts: `.artifacts/reviews/round5-{security,correctness,ux,portability}.md`.

## Iteration 6 finality, resident fencing, and privacy-safe renderer completed

- The v5 migration now discards v4 validators that have no proven effective representation URL; a direct v4 fixture verifies the first v5 selection is unconditional. Manual deadline cancellation seals a command-only unknown tombstone and returns typed `unknown` finality rather than deleting the replay fence. A first committed-source/replay test proves the same ID cannot execute again.
- A scheduled instant has at most two owners. The second unknown is terminal, and scheduler math advances from terminal handled instants (including failure/exhaustion), independently of last success.
- Resident classification, 200 ingest, 304 checkpoints, failure checkpoints, and digest publication validate owner/token/unexpired wall-clock authority at the side-effect boundary. Network/model work is bounded below the ten-minute lease and heartbeats run between those bounded source operations; a stale owner cannot commit after suspend/clock-driven recovery even if its Tokio deadline has not fired. DB tests exercise lease loss between fetch identity and commit, stale side effects, stale finish, recovery, and model cache rollback; a Tokio paused-time test executes the actual eight-minute deadline wrapper. Full end-to-end resident orchestration under native suspend/wake remains an acceptance evidence gap rather than claimed coverage.
- Migration 6 adds a monotonic privacy epoch for retention, source deletion, Not relevant, and Mute. The renderer immediately purges invalidated content while preserving harmless pending-edition reordering, keeps pending editions across unrelated feedback/settings/source mutations, restores focus after Apply, and shows a calm visible operation status alongside the live region.
- Manual sync finality distinguishes complete/partial/unknown. Source-cap-only copy no longer calls unattempted sources failed; null poll time says Eligible now; quiet-hour fields have associated validation guidance; missing canonical URLs remain explicit.
- Secondary-control borders and the dark brand mark now meet tested 3:1 non-text/decorative contrast pairs.

## Deferred foundation work

- Local model inference is active only for the explicit installed-model setting and a four-item global sync budget. GPU qualification, dynamic power/thermal pressure, semantic grounding evaluation, and cancellation UI remain pending.
- The resident runner is process-only. Tray lifecycle, hidden/closed-session helpers, battery/metered signals, backup/export/restore, and OS-level wake scheduling remain pending.
- Stale pending receipts fail closed as command tombstones; this prevents duplicate effects but can conservatively suppress an effect that did not commit before a crash. Physical SQLite/WAL erasure remains outside logical deletion guarantees.
- The conservative special-purpose range policy and connection pinning have unit seams but still need a deterministic mock-transport integration suite and periodic IANA review.
- Native vault round trips, packaged hostile-content/WebView tests, macOS/Linux execution, installers, signing, notarization, SBOM, and clean-host evidence remain release gates.
- Production trend clustering and gated learned ranking remain intentionally absent.

## Iteration 8 final acceptance fixes completed

- Migration 7 repairs validators for databases that already recorded v5, clearing only rows without a proven representation URL. Direct v5→latest coverage proves bound validators survive.
- Runner finish now requires matching, unexpired job and runner authority at the supplied time. An expired owner makes no changes and follows normal bounded recovery. A second owner expiry durably commits `UNKNOWN_RECOVERY_EXHAUSTED`, clears both leases, reports terminal failure, advances handled-instant truth, survives reopen, and permits the next instant rather than a third owner.
- Feedback insertion, the conditional Not relevant/Mute privacy-epoch increment, and receipt completion commit in one transaction. Injected epoch/receipt failures roll back every effect and safely remove only the pending receipt for retry. Undo/reset intentionally reveal content without lowering or resetting the monotonic epoch.
- Renderer mutation generations supersede older polls, and every install checks the highest open/pending privacy epoch. Out-of-order Not relevant and source-delete tests prove stale responses cannot restore removed data. Settings/add/delete/reset preserve a harmless pending edition.
- The selected-model field exposes exact syntax validity/help; partial-result routes use explicit high-contrast secondary controls; schedule conflict and add-source URL failures are associated with their inputs; a working skip link targets focusable main content. Exact cap-only and Eligible-now copy are covered, and reset focus no longer races a stale feedback origin.

Round-6 artifacts: `.artifacts/reviews/round6-{security,correctness,ux,portability}.md`.
Iteration-8 worker artifact: `.artifacts/workers/iteration8-final-acceptance-fixes.md`.

## Iteration 10 final two foundation highs closed

- Trend loading now rejects the whole persisted cluster before selecting its label/summary when any member has active Not relevant feedback or belongs to an actively muted source. Tests cover two-source derivatives, unrelated-member feedback, ranking-only/inactive feedback, Mute, Not relevant, Undo, and reset without epoch decrease. Existing deletion and retention paths already delete every affected cluster before removing contributing posts.
- Add/delete use an exhaustive external-command admission policy. `Unknown` returns a calm non-retry conflict before connector construction/use, secret lookup/deletion, or database mutation; `Complete` returns the current dashboard, and `New` alone executes. Stale-receipt tests preserve sources and show zero feed/vault/database effect probes; local command policies also explicitly suppress Complete/Unknown.

Round-7 artifacts: `.artifacts/reviews/round7-{security,correctness,ux,portability}.md`.
Iteration-10 worker artifact: `.artifacts/workers/iteration10-final-two-highs.md`.

## Round-8 narrow acceptance findings

- Production delete currently transforms a completed receipt to a command-only tombstone. Replace it with a non-correlatable keyed payload binding so the original payload replays Complete but a different source payload conflicts, without retaining raw identifiers.
- Feedback, settings, and reset must not collapse stale `Unknown` into ordinary successful completion. Return typed fail-closed finality (or safely retry only an atomic effect where explicitly proven), and prevent false feedback Undo UI.

Round-8 artifacts: `.artifacts/reviews/round8-{security,correctness}.md`.

## Iteration 11 private receipts and truthful local finality completed

- Migration 8 adds a random, unique replay capability to the durable source tombstone. A completed delete receipt stores only that capability: same request/source replays Complete across reopen, a different source conflicts, and neither request receipts nor audit rows contain a raw source ID or deterministic source-ID digest.
- Source deletion converts linked add/feedback receipts to explicit Unknown tombstones because their erased payloads can no longer be safely proven. Legacy command-only tombstones migrate to Unknown rather than falsely accepting arbitrary payloads as Complete.
- Feedback now returns a typed non-retryable conflict for stale Unknown. Settings and reset share an exhaustive local admission policy where New alone executes, Complete is idempotent replay, and Unknown is a truthful error before effects.
- Direct tests cover the production deletion path across reopen, same/different payload, audit/receipt privacy, source preservation, v7 migration semantics, stale feedback/settings/reset with zero effects, and visible renderer guidance with no false Undo.

Iteration-11 artifact: `.artifacts/workers/iteration11-private-receipts-finality.md`.

## Round-9 foundation acceptance

Round-9 security and correctness reviewers found no blocker/high in migration 8, private production deletion replay, erased-binding Unknown semantics, or feedback/settings/reset finality. The RSS/local-model/open-process foundation is accepted and frozen at blocker/high severity.

Residual boundaries remain explicit: same-user whole-database access can correlate the durable source tombstone mapping; SQLite/WAL/backups are not physically erased or application-encrypted; packaged/native vault, suspend/wake, non-Windows runtime/package, signing/notarization, and clean-host evidence remain release gates.

Round-9 artifacts: `.artifacts/reviews/round9-{security,correctness}.md`.

## Iteration 12 official-provider gates frozen

- Mastodon release baseline is 4.3+ with normalized HTTPS instance origins, proprietary per-instance app registration, PKCE S256/state, an externally opened authorization flow, validated loopback compatibility, exact `profile read:statuses` scopes, OS-vault client/token storage, bounded home/context reads, and provider-instance terms/privacy/rules review. No shared embedded secret or broad read scope is acceptable.
- Bluesky production OAuth cannot be purely infrastructure-free: it requires a public HTTPS client-metadata/policy origin and owned native callback. PAR, PKCE, DPoP, issuer/DID/PDS validation, exact current permission sets, serialized token rotation, moderation parity, and independent-PDS testing are release gates. App passwords remain disabled as a default path.
- The frozen connector expansion order is: platform-neutral bounded batch/comments/health contract; migration and vault-backed OAuth session state; Mastodon behind a disabled validation capability; then Bluesky only after external metadata/callback prerequisites exist.

Artifacts: `.artifacts/research/{mastodon-live-oauth-2026,bluesky-live-oauth-2026,official-connectors-codebase-seams}.md`.

## Iteration 13 neutral connector/comment foundation completed

- The connector trait is now provider-neutral and Rust-only authentication context is non-serializable. A validated batch carries bounded posts/comments, opaque cursor, finality, typed health/retry, completeness/truncation, and RSS-only representation metadata. Caps are 100 posts, 50 comments/post, 500 comments and 256 KiB comment text/batch, 4,000 bytes/comment, depth 8, 8 KiB config, and 2 KiB cursor.
- RSS uses the neutral trait but keeps its accepted DNS/proxy/SSRF/redirect/2 MB/validator/timestamp/item behavior and always reports comments unavailable. No OAuth, browser launch, real token, provider network module, or renderer permission was added.
- Migration 9 adds source health/comment metadata, deterministic comment order fields, and per-post completeness. Generic fenced persistence validates first, then transactionally rechecks generation and optional resident authority before committing cursor, posts, comments, and metadata. Oversize and stale source/owner tests prove no partial effects; existing retention/deletion cascades remain active.
- Backend descriptors expose RSS available, Mastodon validation-required, and Bluesky blocked. The Sources view shows exact unmet prerequisites and read-only limitations without Connect or credential controls; browser fixtures remain explicit demo data.

Iteration-13 artifact: `.artifacts/workers/iteration13-neutral-connectors-comments.md`.

## Round-10 neutral acceptance findings

- Bound and plain-text validate every persisted untrusted author/parent/title field, validate canonical HTTP(S) URLs and the source spec at the generic commit boundary, and use a redacted secret type before provider activation.
- Preserve accepted RSS parser character bounds independently from stricter future-provider byte bounds; add multibyte/GUID regression fixtures.
- Define enforceable page finality and comment completeness combinations. Complete snapshots must reconcile absent comments; partial/comment-only pages must update only truthful state and job finality.
- Bind summaries/provenance to a deterministic comment snapshot plus completeness/truncation. Comment changes, provider deletions, and retention must invalidate derived comment overviews and advance privacy state when visible text is removed.

Artifacts: `.artifacts/reviews/round10-neutral-{security,correctness,ux-portability}.md`.

## Iteration 15 neutral finality/provenance remediation completed

- Provider-specific validation now checks every persisted untrusted string, normalized credential-free HTTP(S) canonical URL, config, cursor, source identity, and batch before transaction entry. Social byte caps remain strict; RSS keeps its original 1,000/500/20,000-character ID/title/body and 200-character author bounds, including multibyte fixtures. Connector secrets are non-serializable, debug-redacted, and zeroed on drop.
- Migration 10 adds source page finality plus per-post comment evidence/summary hashes and allows an explicit partial job state. A batch declares exact comment post scope. Complete untruncated snapshots delete omitted comments only in scope; partial/truncated snapshots merge observed evidence and persist partial source/job truth. Comment-only pages update stored-post state.
- Pre-inference classification compares post plus prospective ordered comment evidence, so changed comments return the stored post and unchanged snapshots consume no model attempts. Summary input/provenance includes completeness, truncation, ordered comment identity, and exact evidence hash. Partial fallback/model requests carry explicit caveats.
- Provider reconciliation and comment-only retention invalidate stale summaries transactionally and advance privacy state. Privacy merges refresh retained item/trend objects while preserving edition order, preventing old comment overviews from remaining visible.

Iteration-15 artifact: `.artifacts/workers/iteration15-neutral-finality-provenance.md`.

## Round-11 neutral acceptance findings

- Add `partial` to the renderer activity schema and visible styling; test a backend-shaped dashboard with a partial job.
- Reject duplicate comment remote IDs and any attempt to reassign a source-wide comment ID to another post, or invalidate both old/new scopes atomically.
- Replace ordinary `Vec::fill` secret cleanup with a maintained non-elidable zeroization primitive.
- Migration 10 must fail closed for v9 non-unavailable comment states/legacy summaries; add direct v9 fixture and migration-specific rollback.
- Enforce the full finality matrix so partial/truncated comment evidence requires partial page/source/job truth.
- Classification must return the exact prospective merged comments/completeness/hash as an immutable inference candidate; preparation and commit must consume that same candidate.

Artifacts: `.artifacts/reviews/round11-neutral-{security,correctness,ux-portability}.md`.

## Iteration 17 neutral activation closure completed

- The Rust/TypeScript activity contract accepts `partial`, and Activity presents it as the calm bounded outcome “partial · more may remain.” Native-shaped Zod and component tests cross the renderer boundary.
- Batch validation rejects duplicate comment remote IDs. Classification and commit both reject an existing source-wide comment ID assigned to another post before durable effects; partial and complete adversarial cases preserve the old comment, state, and summary.
- Connector secrets use maintained `Zeroizing` memory while remaining non-serializable and debug-redacted. This is process-memory hygiene, not protection from a privileged debugger.
- Append-only migration 11 removes legacy social summaries that cannot prove comment evidence identity, marks those snapshots conservative partial/truncated, and preserves valid RSS/unavailable summaries. Direct v9 upgrade and injected migration rollback cover all v9 job states, new columns, repair, reopen, and version atomicity.
- The exhaustive finality matrix requires partial/truncated evidence to persist partial page/source/job truth. Valid combinations have direct cursor/source/job assertions.
- Classification returns immutable candidates containing the prospective post, exact merged sorted comments, completeness/truncation, evidence hash, and combined input hash. Preparation consumes only candidates; commit reclassifies under generation/runner/cursor fences and rejects stale/extra/missing work. An actual fallback-path test proves partial merge sees retained evidence, unchanged evidence uses zero budget, stale work fails, and reopen identity is stable.

Iteration-17 artifact: `.artifacts/workers/iteration17-neutral-final-closure.md`.

## Round-12 final neutral findings

- Preserve source-wide first-seen comment ID→post assignment across reconciliation and retention in a privacy-compatible durable identity table/tombstone; remove only with source deletion. Cover delete/reopen/reassign and retention/reassign.
- Validate prepared summaries as an exact unique set of `(remote_id,input_hash)` keys; duplicate A must not conceal missing B, and rejection must have no source/cursor/comment/summary/job/privacy effects.
- Canonically sort complete candidates with the same helper used by evidence hashing and partial merges before model/fallback preparation. Reversed provider array order must produce identical model input and identity.

Artifacts: `.artifacts/reviews/round12-neutral-{security,correctness,ux-portability}.md`.

## Review status

Twelve review rounds have completed. Neutral UX/portability and iteration-17 migration/finality/partial-path changes pass. Social activation remains disabled until the three identity/set/order findings above close and receive final narrow acceptance.

## Verification evidence

Working directory: the repository root

- Iteration 4 `pnpm verify`: **PASS** — Prettier, ESLint, TypeScript, 10 Vitest tests, Vite production build, Rust format, 32 Rust tests, and Clippy with warnings denied.
- Iteration 4 `pnpm tauri build --no-bundle`: **PASS** on Windows; release binary preserved at `src-tauri/target/release/web-social-digest.exe`.
- Iteration 5 `pnpm verify`: **PASS** — Prettier, ESLint, TypeScript, 13 Vitest tests, Vite production build, Rust format, 39 Rust tests, and Clippy with warnings denied.
- Iteration 5 `pnpm tauri build --no-bundle`: **PASS** on Windows with an empty built-in capability permission list; release binary preserved at `src-tauri/target/release/web-social-digest.exe`.
- Iteration 6 `pnpm verify`: **PASS** — Prettier, ESLint, TypeScript, 15 Vitest tests, Vite production build, Rust format, 45 Rust tests, and Clippy with warnings denied.
- Iteration 6 `pnpm tauri build --no-bundle`: **PASS** on Windows; release binary preserved at `src-tauri/target/release/web-social-digest.exe`.
- Iteration 8 `pnpm verify`: **PASS** — Prettier, ESLint, TypeScript, 23 Vitest tests, Vite production build, Rust format, 49 Rust tests, and Clippy with warnings denied.
- Iteration 8 `pnpm tauri build --no-bundle`: **PASS** on Windows; release binary preserved at `src-tauri/target/release/web-social-digest.exe`.
- Iteration 10 `pnpm verify`: **PASS** — Prettier, ESLint, TypeScript, 23 Vitest tests, Vite production build, Rust format, 52 Rust tests, and Clippy with warnings denied.
- Iteration 10 `pnpm tauri build --no-bundle`: **PASS** on Windows; release binary preserved at `src-tauri/target/release/web-social-digest.exe`.
- Iteration 11 `pnpm verify`: **PASS** — Prettier, ESLint, TypeScript, 24 Vitest tests, Vite production build, Rust format, 54 Rust tests, and Clippy with warnings denied.
- Iteration 11 `pnpm tauri build --no-bundle`: **PASS** on Windows; release binary preserved at `src-tauri/target/release/web-social-digest.exe`.
- Iteration 13 `pnpm verify`: **PASS** — Prettier, ESLint, TypeScript, 25 Vitest tests, Vite production build, Rust format, 58 Rust tests, and Clippy with warnings denied.
- Iteration 13 `pnpm tauri build --no-bundle`: **PASS** on Windows; release binary preserved at `src-tauri/target/release/web-social-digest.exe`.
- Iteration 15 `pnpm verify`: **PASS** — Prettier, ESLint, TypeScript, 25 Vitest tests, Vite production build, Rust format, 61 Rust tests, and Clippy with warnings denied.
- Iteration 15 `pnpm tauri build --no-bundle`: **PASS** on Windows; release binary preserved at `src-tauri/target/release/web-social-digest.exe`.
- Iteration 17 `pnpm verify`: **PASS** — Prettier, ESLint, TypeScript, 27 Vitest tests, Vite production build, Rust format, 67 Rust tests, and Clippy with warnings denied.
- Iteration 17 `pnpm tauri build --no-bundle`: **PASS** on Windows; release binary preserved at `src-tauri/target/release/web-social-digest.exe`.
- No files staged or committed.

## Next step

Close durable comment identity, exact prepared-set equality, and canonical complete-candidate ordering with direct reopen/retention/model-input tests; then perform one final narrow acceptance. OAuth/provider availability remains disabled and externally gated.
