// ── Sender / filename display safety (anti-phishing) ─────────────────────────
//
// Everything a message carries — display name, address, attachment file name —
// is attacker-controlled text. Rendering it as-is lets a sender impersonate
// someone else in three ways this module defends against:
//
//   1. Bidi / invisible control characters. A single U+202E (RIGHT-TO-LEFT
//      OVERRIDE) inside a name or a file name reverses everything after it, so
//      "annexe_gpj.exe" is painted "annexe_exe.jpg" — the reader sees an image,
//      the download is an executable. Isolates (U+2066-2069) and embeddings
//      (U+202A-202D) can also swallow the surrounding text of the row.
//   2. A display name that *is* an address. "Support <admin@banque.fr>" sent
//      from attaquant@evil.tld renders exactly like the legitimate sender in
//      any UI that shows the name and hides the address (mobile, list rows).
//   3. IDN / punycode homographs. "xn--pypal-4ve.com" is "pаypal.com" with a
//      Cyrillic а — visually identical to the real domain.
//
// These functions are PURE (no React, no DOM, no i18n) so they can be reasoned
// about and tested in isolation; the components only consume their verdict.

// ── 1. Neutralizing control and bidi characters ──────────────────────────────

/**
 * Characters removed from any attacker-controlled text before display:
 *  - C0 controls (U+0000-U+0008, U+000B, U+000C, U+000E-U+001F) and DEL/C1
 *    (U+007F-U+009F) — invisible, and some terminals/renderers act on them;
 *  - zero-width and bidi marks: U+200B-U+200F (ZWSP/ZWNJ/ZWJ/LRM/RLM),
 *    U+202A-U+202E (LRE/RLE/PDF/LRO/RLO), U+2060-U+2064, U+2066-U+2069
 *    (isolates + PDI), U+FEFF (BOM).
 * Tab, CR and LF are handled apart: they are folded to a space rather than
 * dropped, so "a\nb" does not become "ab".
 */
const UNSAFE_CHARS =
  // eslint-disable-next-line no-control-regex
  /[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F-\u009F\u200B-\u200F\u202A-\u202E\u2060-\u2064\u2066-\u2069\uFEFF]/g

const LINE_BREAKS = /[\t\r\n]+/g

/** True when the text carries at least one character we would strip — used to
 *  tell the reader that what they see is not literally what was sent. */
export function hasUnsafeChars(raw: string | null | undefined): boolean {
  if (!raw) return false
  UNSAFE_CHARS.lastIndex = 0
  return UNSAFE_CHARS.test(raw)
}

/**
 * Text safe to drop into the DOM: control/bidi characters removed, line breaks
 * folded, runs of spaces collapsed. Naturally right-to-left names (Arabic,
 * Hebrew…) still render correctly — the bidi *algorithm* works on the
 * characters themselves; only the explicit override marks are dropped.
 */
export function sanitizeDisplayText(raw: string | null | undefined): string {
  if (!raw) return ''
  return raw
    .replace(UNSAFE_CHARS, '')
    .replace(LINE_BREAKS, ' ')
    .replace(/ {2,}/g, ' ')
    .trim()
}

/** Same neutralization for an attachment file name, plus the flag telling the
 *  UI to warn: a file name that needed cleaning is a deliberate attack far more
 *  often than an accident. */
export function sanitizeFileName(raw: string | null | undefined): { name: string; altered: boolean } {
  return { name: sanitizeDisplayText(raw), altered: hasUnsafeChars(raw) }
}

// ── 2. Display name impersonating another address ────────────────────────────

/** Deliberately loose: it must catch anything a human would *read* as an
 *  address inside a display name, not validate one. */
const EMAIL_IN_TEXT = /[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)+/g

const normalizeAddress = (a: string) => a.trim().toLowerCase().replace(/^<|>$/g, '')

/**
 * The address a display name pretends to be, when it differs from the address
 * the message was really sent from. Returns null when the name holds no
 * address, or holds only the sender's own (a common, harmless habit).
 */
export function spoofedAddressInName(
  name: string | null | undefined,
  realEmail: string | null | undefined,
): string | null {
  if (!name) return null
  // Run the match on the SANITIZED name: an attacker can otherwise hide the
  // address from the regex with a zero-width space the renderer ignores.
  const clean = sanitizeDisplayText(name)
  const real = normalizeAddress(realEmail ?? '')
  for (const m of clean.matchAll(EMAIL_IN_TEXT)) {
    if (normalizeAddress(m[0]) !== real) return m[0]
  }
  return null
}

// ── 3. IDN / punycode domains ────────────────────────────────────────────────

// RFC 3492 decoder (decode side only). Kept local rather than pulled from a
// dependency: it is 40 lines, and a mail client must not grow its supply chain
// for something this small.
const BASE = 36
const T_MIN = 1
const T_MAX = 26
const SKEW = 38
const DAMP = 700
const INITIAL_BIAS = 72
const INITIAL_N = 128
const MAX_INT = 0x7fffffff

function digitValue(codeUnit: number): number {
  if (codeUnit >= 0x30 && codeUnit <= 0x39) return codeUnit - 0x30 + 26 // '0'-'9' → 26-35
  if (codeUnit >= 0x41 && codeUnit <= 0x5a) return codeUnit - 0x41      // 'A'-'Z' → 0-25
  if (codeUnit >= 0x61 && codeUnit <= 0x7a) return codeUnit - 0x61      // 'a'-'z' → 0-25
  return BASE                                                          // invalid
}

function adaptBias(delta: number, numPoints: number, firstTime: boolean): number {
  let d = firstTime ? Math.floor(delta / DAMP) : delta >> 1
  d += Math.floor(d / numPoints)
  let k = 0
  while (d > ((BASE - T_MIN) * T_MAX) / 2) {
    d = Math.floor(d / (BASE - T_MIN))
    k += BASE
  }
  return k + Math.floor(((BASE - T_MIN + 1) * d) / (d + SKEW))
}

/** Decodes one `xn--…` label to its Unicode form. Null on any malformed or
 *  overflowing input — a domain we cannot decode is displayed as it came. */
export function decodePunycodeLabel(label: string): string | null {
  if (!/^xn--/i.test(label)) return null
  const body = label.slice(4)
  if (!body) return null

  const output: number[] = []
  const delim = body.lastIndexOf('-')
  if (delim > 0) {
    for (let k = 0; k < delim; k++) {
      const c = body.charCodeAt(k)
      if (c > 0x7f) return null
      output.push(c)
    }
  }

  let n = INITIAL_N
  let i = 0
  let bias = INITIAL_BIAS
  let idx = delim > 0 ? delim + 1 : 0
  if (idx >= body.length) return null

  while (idx < body.length) {
    const oldi = i
    let w = 1
    for (let k = BASE; ; k += BASE) {
      if (idx >= body.length) return null
      const digit = digitValue(body.charCodeAt(idx++))
      if (digit >= BASE) return null
      if (digit > Math.floor((MAX_INT - i) / w)) return null
      i += digit * w
      const t = k <= bias ? T_MIN : k >= bias + T_MAX ? T_MAX : k - bias
      if (digit < t) break
      if (w > Math.floor(MAX_INT / (BASE - t))) return null
      w *= BASE - t
    }
    const out = output.length + 1
    bias = adaptBias(i - oldi, out, oldi === 0)
    if (Math.floor(i / out) > MAX_INT - n) return null
    n += Math.floor(i / out)
    i %= out
    // Lone surrogates and out-of-range code points: refuse rather than build a
    // broken string.
    if (n > 0x10ffff || (n >= 0xd800 && n <= 0xdfff)) return null
    output.splice(i, 0, n)
    i++
  }
  return String.fromCodePoint(...output)
}

/** Scripts that look like Latin letter for letter — mixing them inside a single
 *  label is the homograph attack itself, never a real-world domain. */
function mixesLookalikeScripts(text: string): boolean {
  const latin = /[A-Za-z]|\p{Script=Latin}/u.test(text)
  const confusable = /\p{Script=Cyrillic}|\p{Script=Greek}/u.test(text)
  return latin && confusable
}

export interface IdnDomain {
  /** The domain as transmitted, e.g. "xn--pypal-4ve.com". */
  ascii:   string
  /** Its readable form, e.g. "pаypal.com". Falls back to `ascii` when a label
   *  cannot be decoded. */
  unicode: string
  /** The decoded form mixes Latin with Cyrillic/Greek — a homograph. */
  mixedScript: boolean
}

/**
 * Describes the domain of an address when it uses punycode; null for the plain
 * ASCII domains that make up the overwhelming majority of mail.
 */
export function idnDomainOf(email: string | null | undefined): IdnDomain | null {
  if (!email) return null
  const at = email.lastIndexOf('@')
  if (at < 0) return null
  const ascii = sanitizeDisplayText(email.slice(at + 1)).toLowerCase()
  if (!ascii || !/(^|\.)xn--/i.test(ascii)) return null

  let mixedScript = false
  const unicode = ascii
    .split('.')
    .map(label => {
      const decoded = decodePunycodeLabel(label)
      if (decoded == null) return label
      if (mixesLookalikeScripts(decoded)) mixedScript = true
      return decoded
    })
    .join('.')
  return { ascii, unicode, mixedScript }
}

// ── Verdict consumed by the components ───────────────────────────────────────

export interface SenderSafety {
  /** Display name, neutralized. Empty when the message carried none. */
  name:  string
  /** Address, neutralized. */
  email: string
  /** The address the display name impersonates, when it differs from `email`. */
  spoofedAddress: string | null
  /** Punycode domain of the real address, when there is one. */
  idn: IdnDomain | null
  /** The display name carried control/bidi characters we removed. */
  nameAltered: boolean
  /** Anything worth telling the reader about. */
  suspicious: boolean
  /**
   * What a COMPACT surface (list row, subscription line, avatar tooltip) must
   * print. It is the name in the normal case, but falls back to the raw address
   * as soon as the name is deceptive: in a one-line row there is no space for a
   * warning, and showing the address is itself the warning.
   */
  label: string
  /** Full "Nom <adresse>" form, for `title` tooltips. */
  full: string
}

/** Single entry point: everything a component needs to render a sender safely. */
export function analyzeSender(
  name: string | null | undefined,
  email: string | null | undefined,
): SenderSafety {
  const cleanName  = sanitizeDisplayText(name)
  const cleanEmail = sanitizeDisplayText(email)
  const spoofedAddress = spoofedAddressInName(cleanName, cleanEmail)
  const idn = idnDomainOf(cleanEmail)
  return {
    name:  cleanName,
    email: cleanEmail,
    spoofedAddress,
    idn,
    nameAltered: hasUnsafeChars(name),
    suspicious: spoofedAddress != null || idn != null,
    label: spoofedAddress ? (cleanEmail || cleanName) : (cleanName || cleanEmail),
    full:  cleanName && cleanEmail ? `${cleanName} <${cleanEmail}>` : (cleanEmail || cleanName),
  }
}
