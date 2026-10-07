import { Mail } from 'lucide-react'
import SenderAvatar from './SenderAvatar'
import { analyzeSender } from './senderSafety'
import { useMailStore } from '../store'

/** The sender card shown above search results when the query filters on a
 *  single `from:<email>` — Gmail parity: avatar, display name and the address
 *  as a mail link. Clicking the address opens OUR composer prefilled with it
 *  (not the OS mailto handler). */
export default function SearchSenderHeader({ email, name }: { email: string; name?: string | null }) {
  const { setComposeInitial, setComposeOpen } = useMailStore()
  // Card for a searched sender: the address is right beside the name, but the
  // name itself is still attacker-controlled — neutralize it, and let it fall
  // back to the address when it impersonates a different one.
  const safety  = analyzeSender(name, email)
  const display = safety.name || email.split('@')[0]

  const compose = () => {
    setComposeInitial({ to: [{ email, name: name ?? undefined }], cc: [], subject: '', bodyHtml: '' })
    setComposeOpen(true)
  }

  return (
    <div className="flex items-center gap-4 min-w-0 px-3 py-1.5 -mx-3 rounded-lg hover:bg-surface-2 transition-colors">
      <SenderAvatar email={email} name={name} size={32} />
      <div className="text-sm font-medium text-text-primary truncate max-w-64" title={safety.full}>{display}</div>
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
