<!--
  SPDX-FileCopyrightText: 2026 Kubuno contributors
  SPDX-License-Identifier: AGPL-3.0-or-later
-->

# Kubuno Mail

[![License: AGPL v3](https://img.shields.io/badge/License-AGPL_v3-blue.svg)](LICENSE)
![Rust](https://img.shields.io/badge/Rust-edition_2021-orange.svg)
![React](https://img.shields.io/badge/React-19-61dafb.svg)
![Module](https://img.shields.io/badge/Kubuno-module-4D38DB.svg)

**Kubuno Mail — module de messagerie IMAP/SMTP**

A module for [Kubuno](https://github.com/kubuno/core), the self-hosted, libre (AGPLv3) cloud platform.

## Features

- **External accounts over IMAP/SMTP** — connect any mailbox; credentials are encrypted at rest (AES-GCM) and a background worker keeps every account in sync (configurable interval and per-sync fetch cap).
- **Real conversation threading** — messages are grouped by walking the full RFC 5322 `References` chain (plus `In-Reply-To`), so a reply lands in its thread even when the direct parent was never synced; the subject-based fallback only applies to actual `Re:`/`Fwd:` messages, so unrelated mail sharing a subject is never merged.
- **Categorized inbox** — Primary / Promotions / Social / Notifications tabs with unread badges and a preview of the latest unread message. Categories are plain shareable links (`/mail/#category/…`), so browser back/forward and deep links just work.
- **Fast composing** — floating compose window and inline reply/forward with rich-text formatting (web-safe font families, arbitrary pixel sizes), To / Cc / Bcc fields, attachments, scheduled send, undo send and drafts. Replies and forwards embed the quoted original in the editor, ready to be trimmed or annotated.
- **Recipient autocompletion** — suggestions are ranked by usage from a dedicated per-user address index (fed incrementally by sync and outgoing mail — no mailbox scans), merged with the Contacts module when it is installed (discovered dynamically, silently skipped otherwise).
- **Incoming attachments** — parsed during sync, stored on disk (`mail.attachments_dir`) and downloadable from the reader, with an in-app PDF viewer.
- **Triage tools** — stars, Gmail-style importance marker, labels, user-defined filters, archive, move-to-folder, blocked senders, spam reporting (phishing report = block + spam in one action) and one-click unsubscribe driven by the `List-Unsubscribe` header.
- **Reader conveniences** — previous/next conversation, open in a new window, print, view message source, download as `.eml`, and keyboard shortcuts throughout.
- **Deep shell integration** — folder navigation and labels live in the host shell's left panel, "New message" hangs off the shell's global New button, and content copied from other Kubuno modules pastes into the composer as a clean, sanitizer-proof card.

## Architecture

A standalone Rust process that registers with the [core](https://github.com/kubuno/core) at startup; the core proxies its routes (`/api/v1/mail/*`) and serves its runtime-loaded React frontend bundle.

- **Backend** — `src/`: Axum + SQLx (PostgreSQL, schema `mail`); migrations in `migrations/`.
- **Frontend** — `frontend/`: a React bundle built to `entry.js`, consuming `@kubuno/sdk`, `@kubuno/ui` and `@kubuno/drive` from npm (provided by the host at runtime via the import map).

## Install

This module ships in the **all-in-one [Kubuno](https://github.com/kubuno/core) Docker image** (`ghcr.io/kubuno/kubuno`) — the easiest way to self-host a full Kubuno instance (core + every module). See **[kubuno/docker](https://github.com/kubuno/docker)** for `docker compose` instructions.

Native packages are also published on the [GitHub Releases](https://github.com/kubuno/mail/releases) page for every tagged version:

- **Debian/Ubuntu** — `kubuno-mail_*.deb`
- **Fedora / RHEL / openSUSE** — `kubuno-mail-*.rpm`
- **Windows** — `kubuno-mail-setup-*-x64.exe` (installs into an existing Kubuno core installation)
- **macOS** — `kubuno-mail-*.pkg`

To build this module from source, see below.

## Build

**Requirements:** Rust ≥ 1.82, Node.js ≥ 24, PostgreSQL 16.

```bash
cargo build --release                     # → target/release/kubuno-mail
cd frontend && npm ci && npm run build     # → dist/{entry.js, entry.css}
bash build_deb.sh                          # → dist/kubuno-mail_*.deb
```

Platform-specific packages use the same auto-detecting layout as the `.deb`:

```bash
bash build_rpm.sh                          # → dist/kubuno-mail-*.rpm   (needs rpmbuild)
bash build_windows.sh                      # → dist/kubuno-mail-setup-*-x64.exe (needs NSIS)
bash build_macos.sh                        # → dist/kubuno-mail-*.pkg   (run on macOS)
```

CI builds all of them: `build.yml` produces the `.deb`, and `dist.yml` produces the RPM, Windows and macOS packages, attaching everything to the GitHub Release on `v*` tags.

> Shared dependencies come from Kubuno — no `kubuno/core` checkout required:
> - **Rust** — shared crates via tagged git dependencies on `kubuno/core`.
> - **Frontend** — `@kubuno/sdk`, `@kubuno/ui`, `@kubuno/drive` from the `@kubuno` npm scope.

## License

[AGPL-3.0-or-later](LICENSE) © Kubuno contributors.
