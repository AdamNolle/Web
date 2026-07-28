# ADR 0002: SQLite-owned normalized data and durable jobs

**Status:** accepted — 2026-07-26

SQLite is owned by Rust and opened with foreign keys, WAL, `busy_timeout=5000`, and `synchronous=NORMAL`. Numbered migrations run before UI availability. Connector cursor metadata and normalized content are persisted together in the production pipeline; request IDs and job dedupe keys make writes replay-safe. The initial personal corpus uses bounded in-process comparisons rather than a native vector extension.

Raw provider JSON and media binaries are not stored in the foundation schema. Source deletion cascades into actors, posts, comments, summaries, trends, feedback, and digest joins, then records a content-free audit event. The OS vault secret is deleted before database metadata so vault failure is fail-closed. SQLite/WAL and external backups can retain deleted bytes and the UI/docs disclose that limitation.
