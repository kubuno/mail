# Changelog

All notable changes to **kubuno-mail** are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this
project adheres to [Semantic Versioning](https://semver.org/). Entries are added under
`[Unreleased]` **as the change is made**; `_tools/release.sh` stamps them under the version
number at release time, and CI publishes that section as the GitHub Release notes.

## [Unreleased]

### Added

- **Multiple sources and destinations in the search criteria.** The "From"
  field accepts several addresses, OR-combined (any of these senders); the "To"
  field is a full AND/OR rule tree with nested groups — the same builder as the
  "Additional filters" tab, its conditions locked on `to:`. Both round-trip
  with the bar's query text (`(from:a OR from:b)`, parenthesized pure-`to:`
  groups).
- **Gmail-style contact suggestions in address fields of the search panel.**
  From/To criteria and builder rows whose operator takes an address (`cc:`,
  `bcc:`, `deliveredto:`) suggest contacts as you type — avatar, display name
  and address, merged from the contacts module and the mail address index.
- **Logical repetitions are detected and removed when the search is
  validated.** Clicking "Search" simplifies the rule trees: duplicate
  conditions collapse (`label:x OR label:x` → `label:x`), same-combinator
  nesting flattens, single-child groups unwrap, and boolean absorption applies
  (`A OR (A AND B)` → `A`); duplicate sources are deduplicated too.

- **Search results always carry the filter-chip row** — date, "Has attachment",
  "To", "Exclude promotional offers" (new), "Unread" and the "Advanced search"
  link. On a search view the chips refine the committed query (they append to
  it and lift off it); folder views keep their scoped chip bar.
- **Searching on a single sender features that sender above the results** —
  avatar, display name and their address as a mail link that opens the composer
  prefilled with them.
- **The committed search lives in the URL** — `#search/<encoded query>`, Gmail
  style: a search survives reloads, can be deep-linked and follows the browser's
  back/forward. Clearing the search returns the hash to the folder.
- **Condition rows can be reordered by drag and drop.** Every condition and
  group carries a grip; dropping between siblings shows a crisp insertion bar
  (no translucent browser ghost), and dropping onto a group's header — which
  highlights — moves the node *into* that group. Conditions and whole groups
  move freely between root and any group; a group can never be dropped into its
  own descendants.
- **Uniform field typography in the advanced panel.** The free-value inputs
  render at the same 14px as every dropdown (the coarse-pointer 16px anti-zoom
  guard was out-specifying the primitives' size on some devices).
- **The search language accepts an explicit `AND`.** `(-in:spam AND in:trash) OR
  is:subscription` now parses as intended: `AND` is a combinator (juxtaposition
  already meant AND), no longer a literal word to search for.
- **The "Additional filters" tab is now a full recursive condition tree** — the
  standard query-builder model: every group carries one combinator ("all
  conditions (AND)" / "any condition (OR)") and holds conditions, raw
  expressions and nested groups, with "Condition" / "Group" buttons and per-node
  deletion at every level. Any parenthesized AND/OR mix decomposes into editable
  rows, and every edit rewrites the search bar live with correct parentheses.
  Top-level OR chains are kept whole when pre-filling (so `from:a OR from:b` is
  never silently split into a plain "From" field).

- **A new search operator, `is:subscription`.** It matches messages that carry
  a `List-Unsubscribe` header — the same population the "Manage subscriptions"
  view is built on. It combines with every other operator (`from:`, `in:`,
  `OR`, negation, parentheses), and is suggested in the search bar.

### Fixed

- **The condition builder's "Condition" and "Group" text buttons are now icon
  buttons with tooltips** — a lighter group header, same actions.
- **One insertion slot per boundary in the condition builder.** "After row N"
  and "before row N+1" were shown as two distinct drop positions when they are
  the same place; drop targets are now normalized to a single slot per sibling
  boundary, so hovering either side of a boundary marks the exact same bar.
- **Drag-and-drop in the condition builder no longer shivers.** The insertion
  bar is drawn as an absolute overlay (an in-flow bar shifted the row under the
  pointer and made the before/after test oscillate), the drop mark only updates
  when the target boundary actually changes (a state write per dragover event
  re-rendered in a loop), and leaving a child no longer clears the mark (the
  dragleave storm made it flicker).

### Changed

- **The advanced search panel gained an "Additional filters" tab — a real
  condition builder.** Operator fragments from the bar's query (parenthesized
  groups, OR chains, any supported operator) are decomposed into editable rows —
  the classic filter-builder layout: an AND/OR connector from the second row, an
  operator picker over the full supported list, an enumerated or free value, an
  exclusion toggle and a per-row delete, plus "Add a condition". Consecutive OR
  rows serialize back as a parenthesized group, and every edit rewrites the
  search bar live. "Has the words" only ever holds free words. The panel sits in
  two tabs (Criteria / Additional filters), uses the shared UI primitives
  throughout, with uniform typography and no separator rules between fields.
- **The advanced search panel mirrors the search bar — both ways.** Opening it
  decomposes the bar's query into the fields (From, To, Subject, "Has the
  words", size, date, scope…), unknown fragments landing verbatim in "Has the
  words"; editing any field rewrites the bar's text live. Fields and query text
  can no longer disagree.

- **Manage subscriptions, redesigned.** The subscription list now shows each
  sender with a coloured avatar, its address on its own column and a
  plain-language frequency ("More than 20 emails recently", "10-20 emails
  recently", …). Clicking a sender opens the mailbox on the full Gmail-style
  query — `from:X (-in:spam OR in:trash) is:subscription` — everywhere but
  spam, trash included, subscription messages only. "Unsubscribe" is a discreet
  inline action that
  first asks for confirmation — "Stop receiving messages from all of X's mailing
  lists?" — before triggering the `List-Unsubscribe`, matching the familiar
  mail-client layout.




### Fixed


- **A withdrawn dependency is no longer used.** A crate deep in the tree
  (`spin` 0.9.8, pulled in through the HTTP stack) was yanked by its authors.
  No vulnerability was announced, but a withdrawn crate has no business in a
  release; the lockfile now takes the version that replaced it.
- **The package could not be built where `zip` is absent.** The Windows job of
  the continuous integration has no `zip`, so the Windows package was simply lost
  the first time it was attempted — a script failure, not a build failure. The
  builder now falls back to 7-Zip, then to PowerShell.
### Added

- **This module now ships a `.kbpkg`** — the single package format a Kubuno
  server installs by itself, the same file on Linux, Windows and macOS. It
  carries the same binary, interface and manifest as the system packages,
  arranged the way the server expects to find a module on disk, plus a
  `SHA256SUMS` so a copy carried offline can be checked without the catalogue.
  Nothing changes for existing installations: the `.deb`, `.rpm`, `.exe` and
  `.pkg` are still published, and a catalogue that sees both simply prefers the
  new one. It is also the only format the server can unpack without an external
  tool, which is what makes one-click installation possible away from
  Debian-like systems.
### Fixed

- **A built package could be thrown away instead of published.** The job that
  attaches a package to the release waited ten minutes for another workflow to
  create that release, then gave up with "release never appeared — build.yml
  likely failed". The diagnosis was wrong: on a repository whose `.deb` takes
  longer than ten minutes to build, the release simply did not exist yet, and a
  package that had built perfectly was discarded. Four modules reached v0.1.6
  with packages missing for some systems because of it. The job now creates the
  release itself when it is missing, so it no longer depends on another workflow
  finishing first.
### Added

- **Security policy and CI quality gate.** A `SECURITY.md` documents how to
  report vulnerabilities, and a CI workflow enforces `clippy -D warnings`, a
  dependency-vulnerability audit (`cargo audit`) and the frontend typecheck/tests.

- **Automatic address attribution.** Once a primary domain is verified (DNS
  proof and MX), every account without a mailbox on it receives one, and so
  does every new account thereafter. The local part is built from an
  administrator's rule (`Attribution des adresses` settings — e.g.
  `{prenom}.{nom}`, tokens `{prenom} {nom} {p} {n} {username}`), accents folded
  and clashes resolved with a numeric suffix. It never overwrites: an address
  set by hand, or from a previous run, is left untouched, and the whole
  behaviour can be turned off. Reconciled on a timer, so nothing is lost to a
  missed event or a restart.

- Mail now publishes, on the platform's event bus, that a recipient exchanged
  with a correspondent. An address book can collect those into its "Other
  contacts" list; mail itself knows nothing about who listens, and publishing
  never affects delivery.


### Changed

- **Pill-shaped buttons are gone from the interface.** Filter chips, view
  segments, tab selectors and action buttons that were drawn as pills now use the
  same 4 px corner radius as every other button — the shape set them apart for no
  reason other than habit. Round buttons that hold a lone icon, avatars, status
  dots and non-clickable badges keep their shape: a circle around a single glyph
  is not a pill.

- **The folder filter chips are no longer pill-shaped.** « De », « Indifférente »,
  « Contient une pièce jointe », « À » and « Non lu » now use the same 4 px corner
  radius as every other button in the product; the pill set them apart for no
  reason other than habit. Round icon-only buttons (zoom, print) and status
  badges keep their shape — a circle around a lone icon is not the same thing.

- The monospace fallback for message bodies no longer names a Google font that
  was never shipped; it uses DM Mono, which the instance actually serves.

- Internal refactor: the message composer (`ComposeWindow`) was split into
  focused files (`mail-app/compose/`: `SendButton`, `ComposeFormatToolbar`,
  `ComposeActionBar`, shared `parts`). No visible or behavioural change.

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
