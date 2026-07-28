# ADR 0003: Tool-free local inference with deterministic fallback

**Status:** accepted — 2026-07-26

The first model adapter accepts only plain HTTP endpoints on loopback. There is no cloud fallback. Social evidence is serialized beneath an `untrusted_source_data` key and an immutable system policy; no tools, connector authority, network access, filesystem, credentials, or memory writes are available to the reader model. Output uses a bounded JSON schema and is deserialized into a deny-unknown-fields Rust type.

When the configured model is absent or invalid, an extractive deterministic fallback keeps chronological reading and editions usable. Model labels never choose trend membership or cause side effects. Future model downloads require a separate ADR covering source, license, hash/signature, disk/metered-network consent, and offline execution.
