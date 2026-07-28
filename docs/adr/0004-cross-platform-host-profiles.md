# ADR 0004: Cross-platform host capabilities and conservative model profiles

**Status:** accepted — 2026-07-27

Web supports Windows, macOS, and Linux with Tauri 2. Portable Rust owns paths through Tauri's app-data resolver and secrets through `keyring`: Windows Credential Manager, macOS Keychain, or Linux Secret Service. Missing/locked vaults fail closed; there is no file or environment fallback.

The trusted core reports an intentionally small host-capability DTO: OS, architecture, total RAM, logical CPU count, local-runtime reachability, and explicit `unknown` states for GPU, battery, and metered networking when no reliable portable probe exists. It reports one of three **host envelopes**, not guessed model qualifications:

- **CPU/basic:** conservative or extractive fallback, 4K request context.
- **Balanced:** behaviorally ready selected model with measured memory/CPU headroom, 8K maximum profile context.
- **Performance:** behaviorally ready selected model with greater measured headroom, 16K maximum profile context.

A user explicitly names an already-installed Ollama model. Exact name, digest, byte size, parameter-size label, and quantization are reported only when returned by the runtime; Web never infers them from a model name or host tier. Structured readiness requires a bounded generation probe. All profiles use one concurrent generation request and preserve system headroom. They are advisory envelopes, not automatic selection. A recommendation never downloads a model, starts cloud inference, changes the numeric-loopback endpoint, or claims GPU support. Automatic model downloads are outside the accepted architecture.

Platform package/signing evidence is independent: Windows signing, macOS signing/notarization, and Linux packaging/repository testing remain required even when the cross-platform CI compile matrix passes.
