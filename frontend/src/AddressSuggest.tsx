// Recipient autocompletion shared by every address field (To / Cc / Bcc, both
// the floating ComposeWindow and the inline reply/forward composer).
//
// Two merged sources:
//  1. the mail module's own address index (senders/recipients of synced mail,
//     ranked by usage — GET /mail/addresses) ;
//  2. the contacts module, discovered DYNAMICALLY: we just call its API and
//     silently ignore any failure (module absent, not running…), per the
//     polyrepo rule « ne jamais supposer qu'un module est installé ».
import { useEffect, useRef, useState } from 'react'
import { X, Users } from 'lucide-react'
import { Input } from '@ui'
import { useQuery } from '@tanstack/react-query'
import { api } from '@kubuno/sdk'
import { mailApi } from './api'

export interface AddressSuggestion {
  email: string
  name?: string
  /** Present when the suggestion is a recipient group (« liste de diffusion »):
   *  picking it inserts every member as its own chip. `email` is then a synthetic
   *  `group:<id>` key, never a real address. */
  group?: { members: { email: string; name?: string }[] }
}

/** The user's recipient groups (distribution lists), cached across every address
 *  field. Silent [] on error, per the polyrepo « never assume a feature exists ». */
function useRecipientGroups() {
  const { data } = useQuery({
    queryKey: ['mail-recipient-groups'],
    queryFn:  mailApi.listRecipientGroups,
    staleTime: 60_000,
  })
  return data ?? []
}

type ContactField = { value: string; label?: string | null }
type Contact = { display_name?: string | null; emails?: ContactField[] }

async function fromContacts(q: string): Promise<AddressSuggestion[]> {
  try {
    const { data } = await api.get<{ contacts: Contact[] }>('/contacts/contacts', {
      params: { q, limit: 5, filter: 'has_email' },
    })
    return (data.contacts ?? []).flatMap(c =>
      (c.emails ?? []).map(e => ({ email: e.value, name: c.display_name ?? undefined })) as AddressSuggestion[])
  } catch {
    return [] // module contacts absent ou en erreur → dégradation silencieuse
  }
}

/** Suggestions débouncées pour un préfixe de saisie. Contacts d'abord, puis index mail, dédupliqué. */
export function useAddressSuggestions(query: string): AddressSuggestion[] {
  const [items, setItems] = useState<AddressSuggestion[]>([])
  const seq = useRef(0)
  useEffect(() => {
    const q = query.trim()
    if (q.length < 2) { setItems([]); return }
    const mySeq = ++seq.current
    const h = setTimeout(async () => {
      const [contacts, indexed] = await Promise.all([
        fromContacts(q),
        mailApi.suggestAddresses(q).catch(() => [] as { email: string; name: string | null }[]),
      ])
      if (seq.current !== mySeq) return // réponse périmée (frappe plus récente)
      const seen = new Set<string>()
      const merged: AddressSuggestion[] = []
      for (const s of [...contacts, ...indexed]) {
        const key = s.email.toLowerCase()
        if (!key.includes('@') || seen.has(key)) continue
        seen.add(key)
        merged.push({ email: s.email, name: s.name ?? undefined })
        if (merged.length >= 8) break
      }
      setItems(merged)
    }, 180)
    return () => clearTimeout(h)
  }, [query])
  return items
}

/** Champ destinataires complet : chips + saisie + autocomplétion + navigation clavier.
    Utilisé par les champs À / Cc / Cci du ComposeWindow et du composer inline. */
export function RecipientField({ chips, onChange, placeholder }: {
  chips:        AddressSuggestion[]
  onChange:     (next: AddressSuggestion[]) => void
  placeholder?: string
}) {
  const [input, setInput]   = useState('')
  const [active, setActive] = useState(-1)
  const addrSuggestions = useAddressSuggestions(input)
  const groups = useRecipientGroups()
  // Matching groups are proposed FIRST (with a group icon); typing a group's
  // name and selecting it fans out to every member address.
  const q = input.trim().toLowerCase()
  const groupMatches: AddressSuggestion[] = q.length >= 1
    ? groups
        .filter(g => g.name.toLowerCase().includes(q))
        .slice(0, 3)
        .map(g => ({ email: `group:${g.id}`, name: g.name, group: { members: g.members } }))
    : []
  const suggestions = [...groupMatches, ...addrSuggestions]

  const add = (s?: AddressSuggestion) => {
    // A group fans out to all its members (deduped against existing chips).
    if (s?.group) {
      const have = new Set(chips.map(c => c.email.toLowerCase()))
      const additions = s.group.members
        .filter(m => m.email && !have.has(m.email.toLowerCase()))
        .map(m => ({ email: m.email, name: m.name ?? undefined }))
      if (additions.length) onChange([...chips, ...additions])
      setInput(''); setActive(-1)
      return
    }
    const v = s ?? (input.trim().includes('@') ? { email: input.trim() } : undefined)
    if (!v) return
    if (!chips.some(c => c.email.toLowerCase() === v.email.toLowerCase())) {
      onChange([...chips, { email: v.email, name: v.name ?? undefined }])
    }
    setInput(''); setActive(-1)
  }

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (suggestions.length && (e.key === 'ArrowDown' || e.key === 'ArrowUp')) {
      e.preventDefault()
      setActive(a => (a + (e.key === 'ArrowDown' ? 1 : -1) + suggestions.length) % suggestions.length)
      return
    }
    if (e.key === 'Enter' || e.key === ',') {
      e.preventDefault()
      add(active >= 0 ? suggestions[active] : undefined)
      return
    }
    if (e.key === 'Escape' && suggestions.length) { e.stopPropagation(); setActive(-1); setInput(input => input) }
    // Champ vide + retour arrière → retire la dernière pastille (confort Gmail).
    if (e.key === 'Backspace' && !input && chips.length) onChange(chips.slice(0, -1))
  }

  return (
    <div className="relative flex-1 flex flex-wrap gap-1 items-center min-w-0">
      {chips.map((a, i) => (
        <span key={a.email + i} className="flex items-center gap-1 bg-surface-2 text-text-secondary text-xs px-2 py-0.5 rounded-full max-w-xs truncate">
          {a.name ? `${a.name} <${a.email}>` : a.email}
          <button onClick={() => onChange(chips.filter((_, idx) => idx !== i))}><X size={9} /></button>
        </span>
      ))}
      <input
        type="email"
        value={input}
        onChange={e => { setInput(e.target.value); setActive(-1) }}
        onKeyDown={onKeyDown}
        onBlur={() => add()}
        placeholder={chips.length ? '' : placeholder}
        className="flex-1 min-w-28 text-sm outline-none bg-transparent text-text-primary placeholder:text-text-tertiary"
      />
      <AddressSuggestList items={suggestions} activeIndex={active} onPick={s => add(s)} />
    </div>
  )
}

/** Liste déroulante de suggestions, à placer sous le champ de saisie (parent en `relative`). */
export function AddressSuggestList({ items, activeIndex, onPick }: {
  items:       AddressSuggestion[]
  activeIndex: number
  onPick:      (s: AddressSuggestion) => void
}) {
  if (!items.length) return null
  return (
    <div className="absolute left-0 right-0 top-full mt-1 z-50 bg-white border border-border rounded-lg shadow-lg py-1 max-h-64 overflow-y-auto">
      {items.map((s, i) => (
        <button
          key={s.email}
          // pointerdown : avant le blur du champ (sinon le clic est perdu).
          onPointerDown={e => { e.preventDefault(); onPick(s) }}
          className={`w-full flex items-center gap-2.5 px-3 py-1.5 text-left ${i === activeIndex ? 'bg-surface-2' : 'hover:bg-surface-1'}`}
        >
          {s.group ? (
            <>
              <span className="w-7 h-7 rounded-full bg-primary/15 text-primary flex items-center justify-center shrink-0">
                <Users size={14} />
              </span>
              <span className="min-w-0">
                <span className="block text-sm text-text-primary truncate">{s.name}</span>
                <span className="block text-xs text-text-secondary truncate">
                  {s.group.members.length}&nbsp;{s.group.members.length > 1 ? 'destinataires' : 'destinataire'}
                </span>
              </span>
            </>
          ) : (
            <>
              <span className="w-7 h-7 rounded-full bg-primary/15 text-primary text-xs font-medium flex items-center justify-center shrink-0">
                {(s.name || s.email)[0]?.toUpperCase()}
              </span>
              <span className="min-w-0">
                {s.name && <span className="block text-sm text-text-primary truncate">{s.name}</span>}
                <span className="block text-xs text-text-secondary truncate">{s.email}</span>
              </span>
            </>
          )}
        </button>
      ))}
    </div>
  )
}

/** Single-value address input with Gmail-style suggestions (avatar, name,
 *  address) — used by the advanced search panel's De / À / cc… fields. Unlike
 *  RecipientField there are no chips: picking a suggestion fills the field
 *  with the picked address. */
export function AddressSuggestInput({ value, onChange, placeholder, className, style }: {
  value:        string
  onChange:     (v: string) => void
  placeholder?: string
  className?:   string
  style?:       React.CSSProperties
}) {
  const [focused, setFocused] = useState(false)
  const [active, setActive]   = useState(-1)
  // After a pick the field holds the picked address: don't re-suggest it.
  const picked = useRef<string | null>(null)
  const suggestions = useAddressSuggestions(focused && value !== picked.current ? value : '')
  const pick = (sug: AddressSuggestion) => { picked.current = sug.email; onChange(sug.email); setActive(-1) }
  const onKeyDown = (e: React.KeyboardEvent) => {
    if (!suggestions.length) return
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault()
      setActive(a => (a + (e.key === 'ArrowDown' ? 1 : -1) + suggestions.length) % suggestions.length)
    } else if (e.key === 'Enter' && active >= 0) {
      e.preventDefault(); pick(suggestions[active])
    } else if (e.key === 'Escape') {
      e.stopPropagation(); picked.current = value; setActive(-1)
    }
  }
  return (
    <div className={`relative ${className ?? ''}`}>
      <Input
        type="email"
        value={value}
        onChange={e => { picked.current = null; onChange(e.target.value); setActive(-1) }}
        onKeyDown={onKeyDown}
        onFocus={() => setFocused(true)}
        onBlur={() => { setFocused(false); setActive(-1) }}
        placeholder={placeholder}
        className="w-full"
        style={style}
      />
      {focused && <AddressSuggestList items={suggestions} activeIndex={active} onPick={pick} />}
    </div>
  )
}
