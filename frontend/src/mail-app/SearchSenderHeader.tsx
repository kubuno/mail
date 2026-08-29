import { Mail } from 'lucide-react'
import SenderAvatar from './SenderAvatar'
import { useMailStore } from '../store'

/** The sender card shown above search results when the query filters on a
 *  single `from:<email>` — Gmail parity: avatar, display name and the address
 *  as a mail link. Clicking the address opens OUR composer prefilled with it
 *  (not the OS mailto handler). */
export default function SearchSenderHeader({ email, name }: { email: string; name?: string | null }) {
  const { setComposeInitial, setComposeOpen } = useMailStore()
  const display = name || email.split('@')[0]

  const compose = () => {
    setComposeInitial({ to: [{ email, name: name ?? undefined }], cc: [], subject: '', bodyHtml: '' })
    setComposeOpen(true)
  }

  return (
    <div className="flex items-center gap-4 px-6 py-3 border-b border-border flex-shrink-0 bg-surface-0">
      <SenderAvatar email={email} name={name} size={48} />
      <div className="text-sm font-medium text-text-primary truncate max-w-64">{display}</div>
      <button
        type="button"
        onClick={compose}
        className="flex items-center gap-1.5 text-sm text-primary hover:underline min-w-0"
        title={email}
      >
        <Mail size={14} className="flex-shrink-0" />
        <span className="truncate">{email}</span>
      </button>
    </div>
  )
}

/** Every concrete `from:<email>` of a query (deduped, lowercased) — one card
 *  per searched sender, multi-source `(from:a OR from:b)` queries included.
 *  Negated senders and `from:me`-style words don't feature anyone. */
export function searchedSenders(query: string): string[] {
  const out: string[] = []
  const seen = new Set<string>()
  for (const m of query.matchAll(/(?:^|[\s(])(-?)from:"?([^\s()"]+@[^\s()"]+)"?/gi)) {
    if (m[1]) continue
    const email = m[2].toLowerCase()
    if (!seen.has(email)) { seen.add(email); out.push(email) }
  }
  return out
}
