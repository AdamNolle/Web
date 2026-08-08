# Platform support and evidence matrix

This is an evidence ledger, not a compatibility promise. Web is an unsigned local alpha; no host
is currently a released or supported production platform.

| Host / architecture                        | Current level             | Evidence held                                                                                                                                                                                                                                                                                                                                                                                                      | Still required before release                                                                                                                                       |
| ------------------------------------------ | ------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Windows x86_64                             | Local alpha evidence      | Fresh MSI and NSIS bundles; SHA-256 manifest; executable startup and duplicate-instance smoke; release-WebView semantic, default/light/dark plus compact first-run visual captures, Ctrl+K/Escape command-palette, empty-Library saved-export safeguard, visible non-connecting Mastodon metadata-preflight control, restore-dialog focus/Escape, and inspected explicit 200% Reading scale capture on 2026-08-08. | Clean-host install/update/uninstall, native file dialogs, broader WebView/accessibility, vault, offline/sleep-resume, Authenticode, SmartScreen, and updater proof. |
| macOS arm64                                | Configured CI target only | `macos-latest` is declared in CI. The last hosted run was an arm64 macOS runner, but failed before a fresh matrix could establish current evidence.                                                                                                                                                                                                                                                                | Fresh compile/build, package launch, Keychain, WebKit/accessibility, Developer ID signing, notarization, stapling, and update/rollback evidence.                    |
| macOS x86_64                               | Unassessed                | No current local or hosted artifact/runtime evidence.                                                                                                                                                                                                                                                                                                                                                              | Explicit architecture decision plus the same package/runtime/signing proof as arm64.                                                                                |
| Ubuntu x86_64                              | Configured CI target only | `ubuntu-24.04` is declared in CI; an older run passed, but no fresh run validates this working tree.                                                                                                                                                                                                                                                                                                               | Fresh build, package launch, WebKitGTK/Secret Service, native dialogs, offline/sleep-resume, package checksum/signature, and install/update/uninstall evidence.     |
| Other Linux distributions or architectures | Unassessed                | None.                                                                                                                                                                                                                                                                                                                                                                                                              | Explicit support decision and host-native package/runtime/security evidence.                                                                                        |

The cross-platform Rust/Tauri architecture and host-capability policy are described in
[ADR 0004](../adr/0004-cross-platform-host-profiles.md). A successful compile, checksum, or CI
job alone is never evidence of native WebView, vault, package lifecycle, or platform trust.

The Windows WebView smoke is repeatable with
`powershell -ExecutionPolicy Bypass -File scripts/windows-native-smoke.ps1` after building the
release executable and placing matching `tauri-driver` and Microsoft Edge Driver binaries beneath
the ignored `src-tauri/target/tools` directory. Optional `-ScreenshotPath`, `-WindowWidth`,
`-WindowHeight`, `-Theme`, `-ReadingScale`, and `-ReadingScaleScreenshotPath` parameters capture
actual WebView states for review. It proves first-run rendered semantics, the platform-aware command
shortcut, restore-confirmation focus/dismissal, and an explicit persisted Reading scale selection;
it does not interact with native file pickers or prove installer lifecycle.

The primary window enables Tauri's cross-platform page-zoom hotkeys. Its macOS/Linux polyfill has
the narrow `core:webview:allow-set-webview-zoom` capability it requires. Separately, the explicit
Reading scale preference uses the same native webview API; Windows release-WebView smoke selected,
persisted, captured, and visually confirmed its 200% rendering on 2026-08-08. Synthetic Ctrl+Plus
input still cannot expose a measurable WebView2 zoom factor, and macOS/Linux native zoom evidence
remains open.
