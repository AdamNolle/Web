# Connector support and release gates

| Source             | Foundation status              | Honest capability                                                                                                                  |
| ------------------ | ------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------- |
| RSS/Atom           | Implemented                    | User-selected feeds or explicitly advertised feeds discovered from a publisher website                                             |
| X/Instagram export | Implemented manual import      | User-selected official archive file; local history only, never a live connector                                                    |
| Bluesky            | Blocked descriptor only        | Requires public HTTPS client metadata/policy, owned callback, exact scopes, and OAuth tests                                        |
| Mastodon           | Validation-required descriptor | User-initiated metadata compatibility check is implemented; native OAuth, vault lifecycle, and provider acceptance remain required |
| YouTube            | Deferred                       | Subscription uploads and public comments, not personalized Home                                                                    |
| Reddit             | Gated                          | API capability exists; current registration/commercial approval must be confirmed                                                  |
| X                  | Gated                          | Paid/policy-sensitive reverse-chronological timeline only                                                                          |
| Instagram/Facebook | Unsupported for home digest    | Official APIs do not expose an ordinary user's home/following feed                                                                 |
| LinkedIn           | Unsupported for home digest    | Official APIs focus on approved organization/community management                                                                  |
| TikTok             | Unsupported for home digest    | Display API exposes the authorizing creator's own videos, not Following/For You                                                    |

Every connector must document its dated official endpoints, native OAuth + PKCE flow, exact read scopes, quota/cost, attribution, retention/deletion, derived-summary/embedding rules, and app-review status. Missing API access is a product limitation—not authorization for private endpoints, scraping, cookies, stealth, fingerprint manipulation, CAPTCHA bypass, or proxy/identity rotation.

## Official archive import

Archive import is deliberately not a `Connector`: it has no network client, cursor, retry, OAuth,
or scheduled synchronization. A Rust-owned native dialog accepts one extracted X
`data/tweets.js`/`tweet.js` or Instagram `posts_1.json` file at a time. The renderer never sends or
receives a filesystem path.

The disk reader and parser each enforce a 20 MiB ceiling, and one import accepts at most 25,000
entries. This archive-specific envelope is separate from the 100-post live-connector page bound;
the parser streams entries and aborts before allocating entry 25,001. Files above either archive
limit fail explicitly instead of silently dropping their tail.
Malformed entries and exact duplicates are skipped with partial health/finality instead of being
called a complete import; conflicting duplicate identities reject the file before inference or
persistence. The same trimmed label and platform identify a manual re-import, which updates or adds
changed posts without resummarizing unchanged content. A different platform cannot reuse that label.
Import command receipts are replay-safe and commit atomically with the imported source.

Current limitations: ZIP archives are not unpacked, multipart exports require repeated same-label
imports, post-selection progress/cancellation is not implemented, and real native-dialog behavior
still needs packaged end-to-end evidence. A source can be renamed locally without changing its
identity; archive sources remain manual re-import only.

## Provider-neutral read contract

Rust exposes backend-owned connector descriptors; only RSS is `available`. Mastodon is `validation_required` and Bluesky is `blocked`. These are informational states, not Connect controls. The renderer receives no token, connector network access, or provider OAuth/browser-launch permission.

Mastodon's foundation validates the official OAuth authorization-server metadata before a future
connection can be considered: a root HTTPS instance must advertise same-origin authorization,
token, and app-registration endpoints; authorization-code; PKCE `S256`; and either broad `read`
or the minimum granular `read:accounts` plus `read:statuses` scopes. A person can explicitly ask
the native app to read only the fixed same-origin `/.well-known/oauth-authorization-server`
address. That request is proxy-free, DNS/IP-pinned, limited to 64 KiB, and refuses redirects.
The compatibility receipt is not stored; it never registers a client, opens a browser, exchanges a
token, or enables a social account.

The native OAuth foundation has an in-memory PKCE S256 generator: a fresh 384-bit verifier,
43-character challenge, and independent state value. The verifier is non-serializable,
debug-redacted, and zeroized when the attempt ends. Given a dynamically registered client, its
authorization request is limited to authorization-code, S256, the compatible minimum read scope,
and a numeric `127.0.0.1` loopback `/oauth/callback` address.

A native desktop orchestration composes the verified registration endpoint, fixed client
name, exact callback, selected minimum read scope, native browser opener, callback, and bounded
token exchange. Registration and token traffic use the same proxy-free public-DNS/IP-pinned,
64 KiB, no-redirect transport. The renderer can request it only after a successful native metadata
check and a deliberate read-only authorization action; the system browser is opened by Tauri, and
the renderer never receives a credential or provider-network permission.

The callback acceptance rule is equally narrow before any listener exists: only one `code` and one
CSRF `state` parameter on the exact numeric loopback callback are accepted; all other, duplicated,
missing, malformed, or mismatched parameters fail closed. The authorization code is Rust-only,
debug-redacted, and zeroized after the immediate token exchange attempt.

The native loopback primitive binds a single OS-selected `127.0.0.1` port, waits at most five
minutes for one 8 KiB HTTP request, reads only an origin-form `GET`, responds with inert plain text,
and is consumed after that connection. The orchestration keeps the PKCE material in Rust through
the callback and immediate token exchange.

The token exchange is constrained to the verified token endpoint and bound to the registered
client, exact callback, accepted code, and PKCE verifier. It accepts only bounded Bearer tokens
whose returned scope is either absent or exactly the approved read scope; the access token is
Rust-only, debug-redacted, and zeroized after the immediate vault handoff. Before that handoff,
SQLite stores only an opaque pending-cleanup reference. A vault error is Unknown finality rather
than a retry; app startup safely retries deleting that reference. On success, the vault reference,
paused source, and complete request receipt commit as one local transaction. No token reaches
SQLite, IPC, diagnostics, or the renderer.

Every connector batch is read-only and capped at 100 posts, 50 comments per post, 500 comments and 256 KiB of comment text per sync, 4,000 bytes per comment, and depth 8. It carries an opaque bounded cursor, typed health/retry state, page finality, an explicit post scope, and comment completeness/truncation. Persistence validates every stored string, normalized HTTP(S) canonical URL, source config/cursor, and full batch before a transaction, then rechecks source generation, input cursor, and any resident owner/token/expiry while committing posts, comments, cursor, and metadata. Remote comment IDs are source-wide immutable identities: duplicates and attempts to move an existing ID to another post fail before effects. Complete untruncated comment snapshots replace comments only inside their declared post scope; partial/truncated evidence requires partial page/source/job truth and only upserts observed evidence. RSS always reports comments `unavailable`, keeps its original character-based parser bounds (including multibyte text), and retains separate representation-bound HTTP validator state.

Mastodon's current read path is a 40-status home-timeline page with a 1 MiB streamed response cap.
It uses the same proxy-free, public-DNS/IP-pinned, no-redirect transport as metadata and OAuth,
requires an OS-vault token solely in Rust, parses boosts as the boosted status, renders HTML as
inert plain text, and preserves content warnings as titles. It persists only through the generic
source-generation/cursor fence. It fetches context only for at most five timeline roots, capped at
256 KiB per context response; those descendants are stored as explicitly partial evidence while
unfetched roots remain outside the scope. If no context can be read, the batch reports comments
`unavailable`. The source remains paused until provider acceptance and native lifecycle evidence
permit activation.

Classification returns an immutable inference candidate containing the exact prospective post, merged sorted comments, completeness/truncation, evidence hash, and combined input hash. Model/fallback preparation consumes that candidate directly, and commit rejects missing, extra, stale, or cursor-divergent work. Comment-only changes are eligible for the same bounded summary budget; unchanged evidence is not. Provider-deleted or retention-expired comments invalidate derived overviews transactionally and advance the privacy epoch so an open edition receives refreshed retained items without applying unrelated reordering. Migration 11 fails closed for pre-v10 social comment summaries that cannot prove this evidence binding, while RSS/unavailable summaries remain current. Partial fallback copy explicitly says that evidence is incomplete. Connector secrets are Rust-only, non-serializable, debug-redacted, held in `Zeroizing` memory, and absent from SQLite/IPC; zeroization is best-effort process hygiene, not debugger protection.

## RSS synchronization lifecycle

A person may paste either a direct feed URL or a normal publisher page. For a publisher page, Rust
uses the same proxy-free, DNS-pinned, redirect-revalidated transport as synchronization, reads at
most 512 KiB, and returns only explicit `<link rel="alternate">` RSS/Atom advertisements for
review. It does not infer feeds from prose, query a search engine, or let React fetch the page.
Every discovered URL passes the normal public-network checks again when it is connected.

Validators are associated with the final effective representation URL that issued them. Resync sends bounded `ETag`/`Last-Modified` values only to that URL, never across a redirect; valid 304 response validators rotate the checkpoint and invalid/oversized values are discarded. HTTP 304 advances source health without replacing content. HTTP 200 classifies posts by content hash before inference, preserves unchanged summaries/provenance, and atomically commits only new/content-changed summaries with the source checkpoint.

Resident work selects only sources whose `next_poll_at` is due and applies bounded exponential backoff. The explicit “Sync all now” action is a typed manual override of retry timing, still capped at 20 sources/eight minutes. Each live source has a durable generation; ingest requires the exact generation observed before fetch, and deletion writes a tombstone before cascading data so stale work cannot recreate it. Explicit re-add advances the generation.

The process-resident runner uses a renewable ten-minute lease with a unique owner and monotonically increasing fencing token. Heartbeat and finish compare both values, stale finishers are rejected, expired/unknown work can be recovered once, and scheduled work does not create a manual command receipt. It works only while Web is open, respects editable quiet hours, and installs no hidden OS task.

Connected RSS sources can be renamed, paused/resumed, or deliberately synchronized one at a time.
Pause increments the durable source generation, prevents automatic and manual fetches, and leaves
the locally retained edition intact. Resume makes the source eligible again; a per-source sync
remains bounded by the ordinary item/model limits and rebuilds one finite edition afterward.

Any browser-assisted connector requires explicit legal/product approval, user-visible opt-in, a separately sandboxed process, fixed low-frequency budgets, normal platform controls, and a kill switch. None ships in the foundation release.
