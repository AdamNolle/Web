# Connector support and release gates

| Source             | Foundation status              | Honest capability                                                                           |
| ------------------ | ------------------------------ | ------------------------------------------------------------------------------------------- |
| RSS/Atom           | Implemented                    | User-selected publisher feeds; comments are generally unavailable                           |
| Bluesky            | Blocked descriptor only        | Requires public HTTPS client metadata/policy, owned callback, exact scopes, and OAuth tests |
| Mastodon           | Validation-required descriptor | Requires per-instance OAuth compatibility and provider-policy validation before enablement  |
| YouTube            | Deferred                       | Subscription uploads and public comments, not personalized Home                             |
| Reddit             | Gated                          | API capability exists; current registration/commercial approval must be confirmed           |
| X                  | Gated                          | Paid/policy-sensitive reverse-chronological timeline only                                   |
| Instagram/Facebook | Unsupported for home digest    | Official APIs do not expose an ordinary user's home/following feed                          |
| LinkedIn           | Unsupported for home digest    | Official APIs focus on approved organization/community management                           |
| TikTok             | Unsupported for home digest    | Display API exposes the authorizing creator's own videos, not Following/For You             |

Every connector must document its dated official endpoints, native OAuth + PKCE flow, exact read scopes, quota/cost, attribution, retention/deletion, derived-summary/embedding rules, and app-review status. Missing API access is a product limitation—not authorization for private endpoints, scraping, cookies, stealth, fingerprint manipulation, CAPTCHA bypass, or proxy/identity rotation.

## Provider-neutral read contract

Rust exposes backend-owned connector descriptors; only RSS is `available`. Mastodon is `validation_required` and Bluesky is `blocked`. These are informational states, not Connect controls. The renderer receives no token, connector network access, or browser-launch permission.

Every connector batch is read-only and capped at 100 posts, 50 comments per post, 500 comments and 256 KiB of comment text per sync, 4,000 bytes per comment, and depth 8. It carries an opaque bounded cursor, typed health/retry state, page finality, an explicit post scope, and comment completeness/truncation. Persistence validates every stored string, normalized HTTP(S) canonical URL, source config/cursor, and full batch before a transaction, then rechecks source generation, input cursor, and any resident owner/token/expiry while committing posts, comments, cursor, and metadata. Remote comment IDs are source-wide immutable identities: duplicates and attempts to move an existing ID to another post fail before effects. Complete untruncated comment snapshots replace comments only inside their declared post scope; partial/truncated evidence requires partial page/source/job truth and only upserts observed evidence. RSS always reports comments `unavailable`, keeps its original character-based parser bounds (including multibyte text), and retains separate representation-bound HTTP validator state.

Classification returns an immutable inference candidate containing the exact prospective post, merged sorted comments, completeness/truncation, evidence hash, and combined input hash. Model/fallback preparation consumes that candidate directly, and commit rejects missing, extra, stale, or cursor-divergent work. Comment-only changes are eligible for the same bounded summary budget; unchanged evidence is not. Provider-deleted or retention-expired comments invalidate derived overviews transactionally and advance the privacy epoch so an open edition receives refreshed retained items without applying unrelated reordering. Migration 11 fails closed for pre-v10 social comment summaries that cannot prove this evidence binding, while RSS/unavailable summaries remain current. Partial fallback copy explicitly says that evidence is incomplete. Connector secrets are Rust-only, non-serializable, debug-redacted, held in `Zeroizing` memory, and absent from SQLite/IPC; zeroization is best-effort process hygiene, not debugger protection.

## RSS synchronization lifecycle

Validators are associated with the final effective representation URL that issued them. Resync sends bounded `ETag`/`Last-Modified` values only to that URL, never across a redirect; valid 304 response validators rotate the checkpoint and invalid/oversized values are discarded. HTTP 304 advances source health without replacing content. HTTP 200 classifies posts by content hash before inference, preserves unchanged summaries/provenance, and atomically commits only new/content-changed summaries with the source checkpoint.

Resident work selects only sources whose `next_poll_at` is due and applies bounded exponential backoff. The explicit “Sync all now” action is a typed manual override of retry timing, still capped at 20 sources/eight minutes. Each live source has a durable generation; ingest requires the exact generation observed before fetch, and deletion writes a tombstone before cascading data so stale work cannot recreate it. Explicit re-add advances the generation.

The process-resident runner uses a renewable ten-minute lease with a unique owner and monotonically increasing fencing token. Heartbeat and finish compare both values, stale finishers are rejected, expired/unknown work can be recovered once, and scheduled work does not create a manual command receipt. It works only while Web is open, respects editable quiet hours, and installs no hidden OS task.

Any browser-assisted connector requires explicit legal/product approval, user-visible opt-in, a separately sandboxed process, fixed low-frequency budgets, normal platform controls, and a kill switch. None ships in the foundation release.
