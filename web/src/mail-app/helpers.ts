import type { TFunction } from 'i18next'

// ── Helpers ───────────────────────────────────────────────────────────────────

export function formatDate(s: string, t: TFunction, lang: string) {
  const d = new Date(s)
  const now = new Date()
  const diffMs = now.getTime() - d.getTime()
  const diffMin = Math.floor(diffMs / 60_000)
  if (diffMin < 60)   return t('mail_ago_minutes', { count: diffMin })
  const diffH = Math.floor(diffMin / 60)
  if (diffH < 24)     return t('mail_ago_hours', { count: diffH })
  if (d.toDateString() === now.toDateString()) {
    return d.toLocaleTimeString(lang, { hour: '2-digit', minute: '2-digit' })
  }
  if (d.getFullYear() === now.getFullYear()) {
    return d.toLocaleDateString(lang, { month: 'short', day: 'numeric' })
  }
  return d.toLocaleDateString(lang, { month: 'short', day: 'numeric', year: 'numeric' })
}

// Absolute date plus the relative hint Gmail shows: "3 août 2026 11:07 (il y a 3 jours)".
/** "8 août 2026 à 18h08" — the long, spelled-out form we prefer everywhere a
 *  date is shown to a reader. Other locales keep their usual separator. */
export function longDateTime(d: Date, lang: string): string {
  const date = d.toLocaleDateString(lang, { day: 'numeric', month: 'long', year: 'numeric' })
  const time = d.toLocaleTimeString(lang, { hour: '2-digit', minute: '2-digit' })
  return lang.startsWith('fr') ? `${date} à ${time.replace(':', 'h')}` : `${date} ${time}`
}

/** Same as [`longDateTime`] from a `dd/mm/yyyy hh:mm(:ss)` string (what older
 *  quotes carry); returns the input untouched when it is not that shape. */
export function longDateTimeFromFrench(raw: string, lang: string): string {
  const m = raw.trim().match(/^(\d{1,2})\/(\d{1,2})\/(\d{4})[ ,]+(\d{1,2}):(\d{2})(?::(\d{2}))?$/)
  if (!m) return raw
  const d = new Date(+m[3], +m[2] - 1, +m[1], +m[4], +m[5], +(m[6] ?? 0))
  return Number.isNaN(d.getTime()) ? raw : longDateTime(d, lang)
}

export function formatFullDate(s: string, lang: string) {
  const d = new Date(s)
  const abs = longDateTime(d, lang)

  const rtf = new Intl.RelativeTimeFormat(lang, { numeric: 'auto' })
  const diffMs = d.getTime() - Date.now()
  const mins = Math.round(diffMs / 60000)
  const rel =
    Math.abs(mins) < 60      ? rtf.format(mins, 'minute')
    : Math.abs(mins) < 1440  ? rtf.format(Math.round(mins / 60), 'hour')
    : Math.abs(mins) < 43200 ? rtf.format(Math.round(mins / 1440), 'day')
    : null

  return rel ? `${abs} (${rel})` : abs
}

export function folderFromPath(pathname: string): string {
  const seg = pathname.replace(/^\/mail\/?/, '').split('/')[0]
  return seg || 'inbox'
}

// ── Avatar color ─────────────────────────────────────────────────────────────

const AVATAR_COLORS = [
  '#1a73e8','#d93025','#188038','#e37400','#8430ce',
  '#007b83','#e52592','#185abc','#137333','#c5221f',
]
export function avatarColor(name: string): string {
  let h = 0
  for (let i = 0; i < name.length; i++) h = (Math.imul(h, 31) + name.charCodeAt(i)) | 0
  return AVATAR_COLORS[Math.abs(h) % AVATAR_COLORS.length]
}

// True while the user is typing (do not intercept the keyboard shortcuts).
export function isTyping(): boolean {
  const el = document.activeElement as HTMLElement | null
  return !!el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable)
}
export function plainKey(e: KeyboardEvent): boolean {
  return !e.metaKey && !e.ctrlKey && !e.altKey && !isTyping()
}

/** The people a "reply all" would add on top of a plain reply: the original
 *  recipients (To + Cc) minus ourselves and minus the sender, who is already
 *  the reply's own recipient. Empty means "reply all" would send the exact same
 *  message as "reply" — so the action has nothing to offer and is hidden. */
export function replyAllExtras(
  message: { from_email: string; to_addresses?: { email: string; name?: string }[] | null; cc_addresses?: { email: string; name?: string }[] | null },
  myAddresses: string[],
): { email: string; name?: string }[] {
  const mine = new Set(myAddresses.map(a => a.toLowerCase()))
  mine.add(message.from_email.toLowerCase())
  const seen = new Set<string>()
  const out: { email: string; name?: string }[] = []
  for (const a of [...(message.to_addresses ?? []), ...(message.cc_addresses ?? [])]) {
    const email = a?.email?.toLowerCase()
    if (!email || mine.has(email) || seen.has(email)) continue
    seen.add(email)
    out.push({ email: a.email, name: a.name ?? undefined })
  }
  return out
}
