# Web product and implementation plan

## Product contract

Web is a single-user, read-only, local-first Tauri 2 desktop application for Windows, macOS, and a declared Linux support matrix. It creates finite scheduled editions from user-approved sources, summarizes them with a verified local model when available, and always retains a deterministic no-model fallback.

### Invariants

- React is presentation-only. Rust owns network access, SQLite, secrets, scheduling, connectors, inference, exports, and deletion.
- Social and model content is untrusted data and receives no tools, secrets, filesystem, connector, or side-effect capabilities.
- Official APIs are preferred. No stealth, identity/proxy rotation, CAPTCHA bypass, cookie import, credential interception, undocumented endpoint replay, or anti-bot evasion.
- Authenticated connectors use the native OS vault and fail closed when it is unavailable.
- No cloud inference or telemetry fallback. No automatic runtime/model downloads.
- Editions are finite and stable. No infinite scroll, autoplay, streaks, urgency badges, vanity metrics, or passive engagement learning.
- Claims about connectors, local models, scheduling, deletion, encryption, and OS support must match tested behavior.

## Supported-source roadmap

1. **RSS/Atom:** production connector with bounded conditional sync, safe parsing, normalized attribution, and no general comment claim.
2. **Official archive imports:** bounded, local-only X and Instagram personal-data exports. These are manual imports, never described as live connectors.
3. **Mastodon:** official OAuth home timeline and bounded status context after native PKCE/vault/provider-policy gates pass.
4. **Bluesky:** official AT Protocol OAuth timeline and thread context after desktop OAuth/DPoP validation.
5. **YouTube:** later subscriptions-derived uploads/comments connector, never called personalized Home.
6. **Reddit/X APIs:** disabled until access, cost, retention, and commercial terms are approved.
7. **Meta/LinkedIn/TikTok home feeds:** unsupported. No scraping fallback.

## Milestones

### M1 — trustworthy vertical MVP

- Transactional/version-gated SQLite migrations.
- Production state separated from demo fixtures.
- Secure RSS networking with complete reserved-address handling, DNS connection pinning, downgrade rejection, replay-safe commands, conditional resync, and retention.
- Durable scheduled sync/digest runner while the process is active, one catch-up on restart/resume, one owner-fenced recovery per instant, transaction-fenced resident side effects, and honest lifecycle copy.
- Local-model policy with typed capability states, installed-model checks, bounded streaming responses, schema validation, per-summary provenance, and deterministic fallback.
- Complete source deletion, privacy-epoch renderer invalidation, reset learning, safe original links, actionable visible errors, and accessibility/contrast fixes.
- Adversarial tests and one-command verification.

Iteration 4 activated the process-resident portion of M1. Iteration 5 hardens it with generation-fenced source deletion, effective-representation validator binding, due-only resident selection plus typed manual override, changed-item-only model budgeting, pre/post digest attestation, renewable owner/fencing-token leases without scheduled request receipts, bounded whole runs, actual quiet-hour-aware schedule state, and a calm pending-edition renderer check. Tray/closed-app execution, export/backup/restore, cancellation, native-host attestation, and fresh post-fix acceptance review remain open M1/release work.

### M2 — authenticated social MVP

- Provider-neutral read-only batches, typed health/cursors, hard comment bounds, explicit snapshot scope/finality, and generation/runner/cursor-fenced transactional persistence are the first activation gate. Migrations 9–11 add comment state, evidence-bound summary identity, and fail-closed legacy repair. Complete snapshots reconcile only their scope; partial/truncated evidence persists partial page/source/job truth; source-wide comment IDs cannot move between posts; immutable merged inference candidates prevent classification/preparation drift; and RSS retains its prior character bounds. Disabled backend descriptors still prevent social activation.
- Mastodon OAuth + home timeline + bounded thread comments only after instance loopback compatibility and provider policy/legal validation.
- Bluesky OAuth + timeline + bounded threads only after a public HTTPS metadata/policy origin, owned callback, exact permission set, moderation parity, and independent-PDS tests exist.
- Provider-specific retention/attribution/deletion policies and connector contract suites.

### M3 — trends and learning

- Deterministic lexical clustering with cross-source/origin/actor/dedup gates.
- Optional embeddings only after exact runtime/model capability validation.
- Explicit-feedback-only bounded learning with minimum-data gates, 25% chronological/diversity reserve, why-shown explanations, and undo/pause/reset. Export remains part of the P1 backup/export/restore slice.
- LLMs may label validated clusters but never decide membership.

### M4 — release candidates

- Native CI and packaged smoke evidence for Windows, macOS, and a finite Linux matrix.
- Windows signing, macOS Developer ID/notarization/stapling, Linux checksums/provenance, Tauri updater signing, SBOM, dependency/license/secret scans.
- Clean-host install/update/uninstall, keyring, WebView, offline, CPU-only, sleep/wake, and deletion tests.

## Initial support matrix

| Platform | Development target                                                                | Release status                                                                     |
| -------- | --------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------- |
| Windows  | Windows 10/11 x64 with Evergreen WebView2                                         | Compile/test/native launch and unsigned MSI/NSIS verified; signing/updater pending |
| macOS    | Current supported macOS on the native architecture exercised by `macos-latest` CI | CI workflow prepared; upstream run, native package, and keychain evidence pending  |
| Linux    | Ubuntu 24.04 x64, WebKitGTK 4.1, desktop D-Bus Secret Service                     | CI workflow prepared; upstream runtime/package/keyring evidence pending            |

Other Linux distributions are best effort until separately attested.

## Canonical verification

From the repository root:

```text
pnpm verify
```

Native package/signature checks are separate per-host release gates and cannot be attested from one Windows machine.
