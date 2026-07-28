# ADR 0001: Tauri 2 with an authoritative local Rust core

**Status:** accepted — 2026-07-26

## Context

Web handles OAuth tokens, sensitive reading history, untrusted social text, background schedules, and local inference. A normal web application or privileged renderer would make these boundaries difficult to enforce.

## Decision

Use Tauri 2 across Windows, macOS, and Linux. The React webview is presentation-only. Rust exclusively owns SQLite, credentials, connectors, network policy, jobs, scheduling, inference, deletion, and audit events. The renderer receives only bounded serde DTOs through named custom commands. The main renderer capability grants no built-in core permissions; no filesystem, SQL, shell, process, HTTP, menu, tray, path, window, or webview command set is enabled. Remote pages never load in the app webview.

## Consequences

The app is small and keeps privileged behavior reviewable. Rust plus WebView2/WebKit/WebKitGTK add platform-specific build/debugging requirements. Any future browser-assisted connector must run in a separate sandboxed process and cannot weaken the app window's capability set. An Electron rewrite would require explicit architecture approval, not an incremental dependency change.
