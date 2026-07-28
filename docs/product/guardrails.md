# Calm product guardrails

Web optimizes for comprehension and time intentionally saved—not opens, session length, scroll depth, notifications, or click-through.

## Required

- One or two finite scheduled editions a day; stable order while reading; visible end and next edition time.
- Read-only sources, explicit manual refresh, notifications off, no remote media by default.
- Generated text labeled by context and paired with exact source excerpts, author, source, time, and canonical URL.
- `More like this`, `Less`, `Not relevant`, source mute/delete, and immediate undo are explicit local notes. Learned ranking and `Why shown` are active, bounded to explicit More/Less feedback only, gated by a minimum-signal threshold per source, capped by a 25% chronological/diversity reserve, and pausable/resettable in Settings. Export remains disabled until the later gated personalization work is complete.
- A diversity/quiet-source baseline that personalization cannot eliminate.
- Keyboard navigation, visible focus, semantic headings/status, 200% reflow, WCAG AA contrast, reduced motion, and no color-only status.

## Prohibited

Infinite scroll, autoplay, pull-to-refresh, streaks, urgency/red badges, unread pressure, variable-reward reshuffling, engagement-count prominence, passive dwell/open/scroll learning, emotional-vulnerability targeting, protected-trait features, automatic notifications, and model-authored actions.

## Foundation limits

Learned ranking meets that bar (see `src-tauri/src/ranking.rs`) and is active in production. Trend clustering now also meets it (see `src-tauri/src/clustering.rs`): deterministic lexical term-overlap grouping, a cross-source gate (a single source repeating itself is never a trend), a same-source dedup collapse for reposts, and a deterministic fallback label -- no model ever decides cluster membership. The in-app demo/browser-preview fixture cluster remains a separate, clearly-labeled information-architecture demonstration and is never mixed with real digest data.
