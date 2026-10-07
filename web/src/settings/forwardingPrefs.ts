// ── Forwarding preferences (client-side fallback) ─────────────────────────────
//
// The forwarding rules (destinations + keep/archive the local copy) live on the
// server — they must, so incoming mail is re-sent while the user is offline —
// and are loaded/saved through `/mail/forwarding`. This localStorage blob is
// only a fallback used until the server responds (and if it is unreachable), so
// the tab is never empty on first paint.
//
// The POP and IMAP policy USED to live here too, purely client-side with no
// effect. It is now persisted and enforced server-side (`/mail/pop-imap`, table
// `mail.pop_imap_settings`), so those fields were removed from this store.

export interface ForwardAddress {
  id:      string
  email:   string
  enabled: boolean
}

export interface ForwardingPrefs {
  forwardAddresses: ForwardAddress[]
  forwardKeep:      boolean       // keep Kubuno's copy in the inbox when forwarding
}

export const DEFAULT_FORWARDING_PREFS: ForwardingPrefs = {
  forwardAddresses: [],
  forwardKeep:      true,
}

const STORAGE_KEY = 'mail-forwarding-prefs'

export function loadForwardingPrefs(): ForwardingPrefs {
  try {
    const s = localStorage.getItem(STORAGE_KEY)
    if (s) return { ...DEFAULT_FORWARDING_PREFS, ...JSON.parse(s) }
  } catch { /* ignore corrupt/absent blob */ }
  return DEFAULT_FORWARDING_PREFS
}

export function saveForwardingPrefs(prefs: ForwardingPrefs): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(prefs))
  } catch { /* storage full / disabled — nothing we can do */ }
}

/** Best-effort unique id for a forwarding entry (secure-context safe). */
export function newForwardId(): string {
  try {
    if (typeof crypto !== 'undefined' && crypto.randomUUID) return crypto.randomUUID()
  } catch { /* insecure context — fall through */ }
  return `fwd-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
}
