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
import { X } from 'lucide-react'
import { api } from '@kubuno/sdk'
import { mailApi } from './api'

export interface AddressSuggestion {
  email: string
  name?: string
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
  const suggestions = useAddressSuggestions(input)

  const add = (s?: AddressSuggestion) => {
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
          <span className="w-7 h-7 rounded-full bg-primary/15 text-primary text-xs font-medium flex items-center justify-center shrink-0">
            {(s.name || s.email)[0]?.toUpperCase()}
          </span>
          <span className="min-w-0">
            {s.name && <span className="block text-sm text-text-primary truncate">{s.name}</span>}
            <span className="block text-xs text-text-secondary truncate">{s.email}</span>
          </span>
        </button>
      ))}
    </div>
  )
}
