import { useEffect, useState } from 'react'
import { api } from '@kubuno/sdk'
import { avatarColor } from './helpers'

// ── Sender avatar ─────────────────────────────────────────────────────────────
// Three sources, in order of how much they say about the sender:
//   1. the CONTACTS module — the picture the reader themselves filed, purely
//      local, and discovered dynamically (the module may not be installed);
//   2. BIMI — the logo the sender's DOMAIN publishes about itself, resolved and
//      cached by our backend so the browser never talks to a third party;
//   3. the coloured initial, which is always available.
// No source is keyed on the address outside this instance: nothing tells an
// outside service whose mail is being read.

/** "domain|auth" → picture URL (or null), resolved once per session. The
 *  authentication flag is part of the key: the same domain may legitimately
 *  yield a brand logo on an authenticated message and nothing on a spoofed one. */
const domainCache = new Map<string, string | null>()
/** Address → contact picture URL (or null). */
const contactCache = new Map<string, string | null>()

async function fromContacts(email: string): Promise<string | null> {
  const key = email.toLowerCase()
  if (contactCache.has(key)) return contactCache.get(key)!
  let url: string | null = null
  try {
    const { data } = await api.get<{ contacts: Array<{ id: string; avatar_path?: string | null; emails?: Array<{ value: string }> }> }>(
      '/contacts/contacts', { params: { q: email, limit: 5, filter: 'has_email' } },
    )
    const hit = (data.contacts ?? []).find(c =>
      (c.emails ?? []).some(e => e.value?.toLowerCase() === key) && c.avatar_path)
    if (hit) url = `/api/v1/contacts/contacts/${hit.id}/avatar`
  } catch {
    url = null // contacts module absent or failing → silently skipped
  }
  contactCache.set(key, url)
  return url
}

async function fromDomain(email: string, dmarc?: string | null): Promise<string | null> {
  const domain = email.split('@').pop()?.toLowerCase() ?? ''
  if (!domain) return null
  const key = `${domain}|${dmarc ?? ''}`
  if (domainCache.has(key)) return domainCache.get(key)!
  let url: string | null = null
  try {
    // The server decides what may be shown: it withholds a BRAND logo unless
    // `dmarc=pass`, so a spoofed sender cannot borrow a brand's identity.
    const res = await api.get(`/mail/avatar`, { params: { email, dmarc }, responseType: 'blob' })
    if (res.status === 200) {
      const blob = res.data as Blob
      // A sender hosted by this instance answers with JSON pointing at the
      // OWNER's own profile picture, served by the core.
      if (blob.type.includes('application/json')) {
        const { avatar_url } = JSON.parse(await blob.text()) as { avatar_url?: string }
        url = avatar_url ?? null
      } else {
        url = URL.createObjectURL(blob)
      }
    }
  } catch {
    url = null // 404 (nothing to show) or network trouble → coloured initial
  }
  domainCache.set(key, url)
  return url
}

/**
 * Avatar of a sender: their contact picture, else their domain's logo, else the
 * coloured initial. `size` is the square side in pixels.
 */
export default function SenderAvatar({ email, name, size = 40, dmarc }: {
  email: string
  name?: string | null
  size?: number
  /** DMARC verdict of the message this sender wrote; only 'pass' unlocks the
   *  brand's own logo. Absent (list rows, legacy mail) = not authenticated. */
  dmarc?: string | null
}) {
  const display = name || email
  const initial = display[0]?.toUpperCase() ?? '?'
  const [src, setSrc] = useState<string | null>(null)

  useEffect(() => {
    let alive = true
    setSrc(null)
    if (!email) return
    ;(async () => {
      const found = (await fromContacts(email)) ?? (await fromDomain(email, dmarc))
      if (alive) setSrc(found)
    })()
    return () => { alive = false }
  }, [email, dmarc])

  const box = { width: size, height: size }

  if (src) {
    return (
      <img
        src={src}
        alt=""
        aria-hidden="true"
        draggable={false}
        style={{ ...box, objectFit: 'contain', background: '#fff' }}
        className="rounded-full flex-shrink-0 select-none border border-[#e8eaed]"
        // A logo that fails to decode must not leave a broken image behind.
        onError={() => setSrc(null)}
      />
    )
  }

  return (
    <div
      style={{ ...box, backgroundColor: avatarColor(display), fontSize: Math.round(size * 0.375) }}
      className="rounded-full flex items-center justify-center font-semibold text-white flex-shrink-0 select-none"
    >
      {initial}
    </div>
  )
}
