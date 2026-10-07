// User signatures — stored client-side (like the other mail preferences). A user
// can keep several named signatures and mark one default; the composer offers a
// menu to insert one, and the settings page manages the list.

export interface Signature {
  id:   string
  name: string
  html: string
}

const LIST_KEY = 'mail-signatures'
const DEFAULT_KEY = 'mail-default-signature'
// Per-sending-address signature defaults (Gmail parity): which signature to
// pre-fill for a given « De » address, split by message kind. Keyed by the
// address in lowercase.
const PER_ADDRESS_KEY = 'mail-signature-defaults'

/** Signature ids chosen for one sending address. An empty string is a deliberate
 *  « no signature »; an absent key means « fall back to the global preference ». */
export interface AddressSignatureDefault {
  new?:   string
  reply?: string
}
export type SignatureDefaults = Record<string, AddressSignatureDefault>
export type SignatureKind = 'new' | 'reply'

export function loadSignatures(): Signature[] {
  try {
    const s = localStorage.getItem(LIST_KEY)
    if (s) return JSON.parse(s) as Signature[]
  } catch { /* ignore */ }
  return []
}

export function saveSignatures(list: Signature[]): void {
  try { localStorage.setItem(LIST_KEY, JSON.stringify(list)) } catch { /* ignore */ }
}

export function defaultSignatureId(): string {
  try { return localStorage.getItem(DEFAULT_KEY) ?? '' } catch { return '' }
}

export function setDefaultSignatureId(id: string): void {
  try { localStorage.setItem(DEFAULT_KEY, id) } catch { /* ignore */ }
}

// ── Per-address signature defaults ────────────────────────────────────────────

export function loadSignatureDefaults(): SignatureDefaults {
  try {
    const s = localStorage.getItem(PER_ADDRESS_KEY)
    if (s) return JSON.parse(s) as SignatureDefaults
  } catch { /* ignore */ }
  return {}
}

export function saveSignatureDefaults(map: SignatureDefaults): void {
  try { localStorage.setItem(PER_ADDRESS_KEY, JSON.stringify(map)) } catch { /* ignore */ }
}

/**
 * Resolve which signature id to insert, in the order the users expect:
 *   1. the default set for the ACTIVE sending address (new/reply kind) ;
 *   2. else the global new/reply preference (`prefFallback`) ;
 *   3. else the single « default signature » toggle.
 * A per-address key explicitly set to '' means « none » and short-circuits the
 * fallbacks, so a user can silence the signature for one address only.
 */
export function resolveSignatureId(
  email: string | undefined,
  kind: SignatureKind,
  prefFallback: string,
): string {
  const perAddr = email ? loadSignatureDefaults()[email.toLowerCase()] : undefined
  if (perAddr && kind in perAddr) return perAddr[kind] ?? ''
  if (prefFallback) return prefFallback
  return defaultSignatureId()
}

/** A fresh id — `crypto.randomUUID` throws outside a secure context, so guard it. */
export function newSignatureId(): string {
  try {
    if (typeof crypto !== 'undefined' && crypto.randomUUID) return crypto.randomUUID()
  } catch { /* ignore */ }
  return `sig-${Date.now()}-${Math.floor(Math.random() * 1e6)}`
}
