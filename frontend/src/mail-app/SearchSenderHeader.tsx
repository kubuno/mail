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

/** The single concrete `from:<email>` of a query, or null (absent, `from:me`,
 *  several senders, or part of an OR alternative — no one sender to feature). */
export function searchedSender(query: string): string | null {
  const matches = [...query.matchAll(/(?:^|[\s(])-?from:"?([^\s()"]+@[^\s()"]+)"?/gi)]
  if (matches.length !== 1) return null
  const m = matches[0]
  if (m[0].trimStart().startsWith('-')) return null
  // Inside an OR alternative the sender is not a guaranteed filter.
  const idx = m.index ?? 0
  const around = query.slice(Math.max(0, idx - 4), idx + m[0].length + 4).toUpperCase()
  if (around.includes(' OR ')) return null
  return m[1].toLowerCase()
}
