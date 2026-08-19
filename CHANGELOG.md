# Changelog

All notable changes to **kubuno-mail** are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this
project adheres to [Semantic Versioning](https://semver.org/). Entries are added under
`[Unreleased]` **as the change is made**; `_tools/release.sh` stamps them under the version
number at release time, and CI publishes that section as the GitHub Release notes.

## [Unreleased]

## [0.1.6] - 2026-08-19

### Changed

- Theme tokens: two colours for navigation labels (`--color-text-nav`,
  `--color-text-nav-active`). Every module carries the same token sheet, so the
  values must match across them — whichever bundle loads last would otherwise
  win. No visible change inside this module.
- The search bar follows the new default background (`#e9eef6`).
- Default application background token aligned with the core (`--body-bg` `#f8fafd`). Only
  visible when the module runs standalone: inside the shell the active theme sets it.
- Search bar background now comes from the `--color-search-bg` token (unified at `#e9eef6`)
  rather than a hard-coded colour.

[Unreleased]: https://github.com/kubuno/mail/compare/v0.1.6...HEAD
[0.1.6]: https://github.com/kubuno/mail/releases/tag/v0.1.6
