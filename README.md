<!--
  SPDX-FileCopyrightText: 2026 Kubuno contributors
  SPDX-License-Identifier: AGPL-3.0-or-later
-->

<div align="center">

<img src=".github/logo.png" alt="Kubuno Mail logo" width="120">

# Kubuno — Mail

[![License: AGPL v3](https://img.shields.io/badge/License-AGPL_v3-blue.svg)](LICENSE)
![Rust](https://img.shields.io/badge/Rust-edition_2021-orange.svg)
![React](https://img.shields.io/badge/React-19-61dafb.svg)
![Module](https://img.shields.io/badge/Kubuno-module-4D38DB.svg)
![Status](https://img.shields.io/badge/status-alpha-yellow.svg)

**A full IMAP/SMTP e-mail client for [Kubuno](https://github.com/kubuno/core) — the self-hosted, libre (AGPLv3) cloud platform, a sovereign alternative to Google Workspace and Microsoft 365.**

Connect any mailbox, compose with rich formatting and OpenPGP, triage with labels
and filters, and answer calendar invitations without leaving your inbox.

</div>

---

## ✨ Features

- 📥 **External accounts over IMAP/SMTP** — connect any mailbox; credentials are encrypted at rest (AES-GCM) and a background worker keeps every account in sync (configurable interval and per-sync fetch cap). The instance's own IMAP/POP3 server enforces per-user access controls.
- 🧵 **Real conversation threading** — messages are grouped by walking the full RFC 5322 `References` chain (plus `In-Reply-To`), so a reply lands in its thread even when the direct parent was never synced; the subject-based fallback applies only to actual `Re:`/`Fwd:` messages.
- 🗂️ **Categorized inbox** — Primary / Promotions / Social / Notifications tabs with unread badges and a preview of the latest unread message; each category is a plain shareable link, so back/forward and deep links just work.
- ✍️ **Fast composing** — a floating composer (with a full-screen mode) and inline reply/forward with rich-text formatting, To / Cc / Bcc, scheduled send, undo send, drafts, and quoted originals embedded ready to trim. The compose menu also offers an encrypted (OpenPGP) message, a message from a saved template, and quick actions to create a label, a filter, a distribution list or a scheduled message.
- 📎 **First-class attachments** — full-width attachment bands with per-file encode progress and cancel, drag-and-drop onto the composer, in-place preview/download, and files over the admin size limit transparently uploaded to the user's **Drive** and sent as a link instead of a bounced oversized message.
- 🔐 **OpenPGP / GPG** — per-user keys (generated in-app or imported), sign and/or encrypt outgoing mail (PGP/MIME, RFC 3156), automatic public-key discovery over **WKD** and **Autocrypt**, and decryption + signature verification of incoming mail.
- ✒️ **Signatures & identities** — several named signatures with a default per sending address (distinct for new mail vs replies), verified **send-as** identities, account **delegation** (send *on behalf of* another user), and a loop-safe vacation auto-responder (RFC 3834).
- 👤 **Contact @mentions & autocompletion** — typing `@` suggests contacts (from the Contacts module when installed) and inserts a removable chip; recipient suggestions are ranked from a per-user address index, merged with Contacts when present.
- 📅 **Calendar invitations** — invitation e-mails carry an `.ics`, are laid out like a calendar entry, and offer Yes / No / Maybe (with an optional reason or counter-proposal); answering sends a standards-compliant iMIP reply and mirrors the event into the Calendar module, and incoming RSVPs update the organizer's calendar automatically. Rich cards also surface schema.org data (flights, hotels, orders, parcels…) with "Add to calendar".
- 🔍 **Powerful search** — a query language in the URL (`#search/…`, deep-linkable, back/forward-aware) and a full recursive condition builder with AND/OR groups, drag-and-drop reordering, address autocompletion and live two-way sync with the search bar.
- 🧹 **Triage tools** — stars, importance markers, labels, user-defined filters, archive, move-to-folder, blocked senders, spam/phishing reporting, and one-click unsubscribe driven by the `List-Unsubscribe` header.
- 🛡️ **Hardened rendering** — a vetted HTML sanitiser (no scripts, `data:`/`cid:` only as image sources, e-mail CSS scrubbed), external images held back until you ask, spoofed display names and punycode domains flagged, and trust indicators marked unverified when nothing was actually checked.
- 🔗 **Deep shell integration** — folders and labels live in the host shell's left panel, "New message" hangs off the shell's global New button, and content copied from other Kubuno modules pastes into the composer as a clean card.

## 🏗️ Architecture

Mail is a **Kubuno module**: a standalone Rust process (port `3111`) that registers with the [core](https://github.com/kubuno/core) at startup. The core proxies its routes (`/api/v1/mail/*`) and serves its runtime-loaded frontend bundle.

```
core (kubuno/core)  ──proxy──►  kubuno-mail (this repo, :3111)
       │                              ├─ Rust backend (Axum + PostgreSQL, schema `mail`)
       └─ serves /modules/mail/entry.js (React frontend, loaded at runtime)
```

- **Backend** — `src/`: Axum + SQLx (PostgreSQL, schema `mail`); migrations in `migrations/`.
- **Frontend** — `frontend/`: a React bundle built to `entry.js`, consuming `@kubuno/sdk`, `@kubuno/ui` and `@kubuno/drive` from npm (provided by the host at runtime via the import map).

## 📥 Install

Modules install as a **Kubuno package (`.kbpkg`)** — a single, self-contained archive the Kubuno server unpacks itself (in pure Rust, identically on Linux, Windows and macOS). There are no native system packages for a module; only the core ships those.

The easiest way to self-host a full Kubuno instance (core + every module) is the **all-in-one [Docker image](https://github.com/kubuno/docker)** (`ghcr.io/kubuno/kubuno`), which already bundles Mail.

To build and install this module on its own:

```bash
bash build_kbpkg.sh --install        # build → install into the store → restart the core
```

Or install a prebuilt `.kbpkg` (offline, no catalogue required):

```bash
sudo kubuno modules:install dist/mail-<version>-<os>-<arch>.kbpkg
sudo systemctl restart kubuno        # the core loads the module on (re)start
```

A `.kbpkg` is attached to every tagged [GitHub Release](https://github.com/kubuno/mail/releases) (Linux via `build.yml`, Windows/macOS via `dist.yml`).

## 🛠️ Build & development

**Requirements:** Rust ≥ 1.82, Node.js ≥ 24, PostgreSQL 16.

```bash
cargo build --release                      # → target/release/kubuno-mail
cd frontend && npm ci && npm run build      # → dist/{entry.js, entry.css}
bash build_kbpkg.sh                         # → dist/mail-<version>-<os>-<arch>.kbpkg
```

> Shared dependencies come from Kubuno — no `kubuno/core` checkout required:
> - **Rust** — shared crates via tagged git dependencies on `kubuno/core`.
> - **Frontend** — `@kubuno/sdk`, `@kubuno/ui`, `@kubuno/drive` from the `@kubuno` npm scope. They are `external` at runtime (the host provides the singletons via its import map); the npm packages supply the build-time type surface.

## 📦 Tech stack

Rust 2021 · Axum 0.7 · Tokio · SQLx 0.8 (PostgreSQL, schema `mail`) · aes-gcm · OpenPGP — React 19 · TypeScript · Vite · Tailwind CSS v4 · Zustand · React Query.

## 🤝 Contributing

Issues and pull requests are welcome. For any significant change, please open an issue first.

## 📄 License

[AGPL-3.0-or-later](LICENSE) © Kubuno contributors.
