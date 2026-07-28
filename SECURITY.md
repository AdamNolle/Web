# Security policy

Web is a single-user, local-first desktop application. There is no
multi-tenant server, no telemetry, and no cloud inference — the bulk of
its risk surface is the Rust core's network/SQLite/secrets boundary and
the untrusted content it ingests. See
[`docs/security/threat-model.md`](docs/security/threat-model.md) for the
full asset/boundary breakdown, current controls, and residual risk this
project tracks deliberately rather than claims are fixed.

## Reporting a vulnerability

Please report suspected vulnerabilities privately rather than opening a
public issue. Use GitHub's
[private vulnerability reporting](https://github.com/AdamNolle/web/security/advisories/new)
for this repository, or email the address on the maintainer's GitHub
profile. Include:

- A description of the issue and its impact.
- Steps to reproduce, or a minimal proof of concept.
- The affected version/commit.

You should receive an initial response within a few days. This is a
single-maintainer hobby project, not a funded security program — there
is no bug bounty and no guaranteed SLA, but genuine reports are taken
seriously and credited in the fix.

## Supported versions

Pre-1.0: only the latest commit on `main` is supported. There are no
long-term-support branches yet.

## Scope

In scope: the Rust core (`src-tauri/`), the RSS/Atom connector, the
local-model (Ollama) integration, SQLite persistence/migrations, the
OS-vault credential boundary, and the React renderer's IPC surface.

Out of scope: vulnerabilities that require the attacker to already have
code execution as the same OS user (this app has no elevated privilege
and no cross-user boundary to defend), and denial-of-service against a
single-user local process.
