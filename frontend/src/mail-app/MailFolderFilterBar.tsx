// Gmail-style filter chip bar shown under the toolbar of certain folder views
// (starred, important, sent, drafts, all mail, spam). Each chip refines a search
// scoped to the folder: picking one builds a query — base scope operator plus the
// active chips — and applies it, so the list becomes a filtered search. Clearing
// every chip drops back to the plain folder view. « Recherche avancée » hands the
// current values to the header search's advanced panel.
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { useTranslation } from 'react-i18next'
import { ChevronDown, Check } from 'lucide-react'
import { useMailStore } from '../store'
import { useAddressSuggestions } from '../AddressSuggest'

// ── Which folders get the bar, and with which base scope / chip set ───────────
export type FilterFolder = 'starred' | 'important' | 'sent' | 'all' | 'spam' | 'drafts'

type ChipId = 'from' | 'date' | 'attach' | 'noAgenda' | 'to' | 'unread'

interface FolderCfg {
  /** Search operator that scopes the results to this folder (null = drafts, which
   *  filters its local list client-side instead of searching). */
  scope: string | null
  chips: ChipId[]
}

// Base set is [from, date, attach, noAgenda, to, unread]; each folder drops some.
export const FOLDER_FILTERS: Record<FilterFolder, FolderCfg> = {
  starred:   { scope: 'is:starred',   chips: ['from', 'date', 'attach', 'noAgenda', 'to', 'unread'] },
  important: { scope: 'is:important',  chips: ['from', 'date', 'attach', 'to', 'unread'] },              // no noAgenda
  sent:      { scope: 'in:sent',       chips: ['date', 'attach', 'to', 'unread'] },                      // no from, no noAgenda
  all:       { scope: 'in:anywhere',   chips: ['from', 'date', 'attach', 'noAgenda', 'to', 'unread'] },
  spam:      { scope: 'in:spam',       chips: ['from', 'date', 'attach', 'to', 'unread'] },              // no noAgenda
  // Drafts have no read state and no real recipients-from-others: date, attachment
  // and « À » are the ones that make sense (matches Gmail's drafts filter bar).
  drafts:    { scope: null,            chips: ['date', 'attach', 'noAgenda', 'to'] },                    // no from, no unread
}

export interface FilterState {
  from:    string
  to:      string
  date:    '' | '7d' | '1m' | '6m' | '1y' | 'custom'
  after:   string   // ISO yyyy-mm-dd, for custom range
  before:  string
  attach:  boolean
  noAgenda: boolean
  unread:  boolean
}

export const EMPTY_FILTER: FilterState = {
  from: '', to: '', date: '', after: '', before: '', attach: false, noAgenda: false, unread: false,
}

/** Build the search query for a folder scope + chip values (used by ThreadList folders). */
export function buildFilterQuery(scope: string | null, f: FilterState): string {
  const parts: string[] = []
  if (scope) parts.push(scope)
  if (f.from.trim()) parts.push(`from:${f.from.trim()}`)
  if (f.to.trim())   parts.push(`to:${f.to.trim()}`)
  if (f.date === 'custom') {
    if (f.after)  parts.push(`after:${f.after.replace(/-/g, '/')}`)
    if (f.before) parts.push(`before:${f.before.replace(/-/g, '/')}`)
  } else if (f.date) {
    parts.push(`older_than:${f.date}`)
  }
  if (f.attach)   parts.push('has:attachment')
  if (f.noAgenda) parts.push('-filename:.ics')
  if (f.unread)   parts.push('is:unread')
  return parts.join(' ')
}

/** True when at least one chip carries a value. */
export function isFilterActive(f: FilterState): boolean {
  return !!(f.from || f.to || f.date || f.attach || f.noAgenda || f.unread)
}

// ── Popover anchored under its trigger ────────────────────────────────────────
// Rendered through a portal with fixed positioning: the filter bar is horizontally
// scrollable (overflow-x-auto, which forces overflow-y to clip), so an in-flow
// absolute dropdown would be cut off and slide behind the message list. A portal
// escapes that and floats above everything.
function Popover({ anchorRef, open, onClose, children }: {
  anchorRef: React.RefObject<HTMLElement | null>
  open: boolean
  onClose: () => void
  children: React.ReactNode
}) {
  const ref = useRef<HTMLDivElement>(null)
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null)

  useLayoutEffect(() => {
    if (!open || !anchorRef.current) return
    const place = () => {
      const r = anchorRef.current!.getBoundingClientRect()
      // Clamp so a wide popover near the right edge stays on screen.
      const left = Math.min(r.left, window.innerWidth - 260)
      setPos({ left: Math.max(8, left), top: r.bottom + 4 })
    }
    place()
    window.addEventListener('scroll', place, true)
    window.addEventListener('resize', place)
    return () => { window.removeEventListener('scroll', place, true); window.removeEventListener('resize', place) }
  }, [open, anchorRef])

  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent) => {
      const target = e.target as Node
      if (ref.current?.contains(target) || anchorRef.current?.contains(target)) return
      onClose()
    }
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') onClose() }
    document.addEventListener('mousedown', onDown)
    document.addEventListener('keydown', onKey)
    return () => { document.removeEventListener('mousedown', onDown); document.removeEventListener('keydown', onKey) }
  }, [open, onClose, anchorRef])

  if (!open || !pos) return null
  return createPortal(
    <div
      ref={ref}
      style={{ position: 'fixed', left: pos.left, top: pos.top, zIndex: 1000 }}
      className="min-w-[240px] rounded-lg border border-border bg-surface-0 shadow-lg py-1.5"
    >
      {children}
    </div>,
    document.body,
  )
}

// A pill chip. `active` highlights it (a value is set); `hasCaret` shows the ▾.
function Chip({ label, active, hasCaret, onClick }: {
  label: string; active: boolean; hasCaret?: boolean; onClick: (e: React.MouseEvent) => void
}) {
  return (
    <button
      onClick={onClick}
      className={`inline-flex items-center gap-1 h-8 px-3 rounded-full border text-[13px] whitespace-nowrap flex-shrink-0 transition-colors
        ${active
          ? 'bg-primary/10 border-primary/40 text-primary'
          : 'bg-surface-0 border-border text-text-secondary hover:bg-surface-1'}`}
    >
      {label}
      {hasCaret && <ChevronDown size={14} className="-mr-0.5" />}
    </button>
  )
}

// ── Address chip (De / À) : a text field with contact suggestions ─────────────
function AddressChip({ label, value, onApply }: { label: string; value: string; onApply: (v: string) => void }) {
  const [open, setOpen] = useState(false)
  const [input, setInput] = useState(value)
  const anchorRef = useRef<HTMLDivElement>(null)
  useEffect(() => { setInput(value) }, [value])
  const suggestions = useAddressSuggestions(open ? input : '')
  const commit = (v: string) => { onApply(v.trim()); setOpen(false) }
  return (
    <div className="relative flex-shrink-0" ref={anchorRef}>
      <Chip
        label={value ? `${label} : ${value}` : label}
        active={!!value}
        hasCaret
        onClick={() => setOpen(o => !o)}
      />
      <Popover anchorRef={anchorRef} open={open} onClose={() => setOpen(false)}>
        <div className="px-2 pb-1.5">
          <input
            autoFocus
            value={input}
            onChange={e => setInput(e.target.value)}
            onKeyDown={e => { if (e.key === 'Enter') commit(input) }}
            placeholder={label}
            className="w-full h-9 px-2 text-sm rounded border border-border bg-surface-0
                       focus:outline-none focus:border-primary"
          />
        </div>
        <div className="max-h-64 overflow-y-auto">
          {suggestions.map(s => (
            <button
              key={s.email}
              onClick={() => commit(s.email)}
              className="flex flex-col items-start w-full px-3 py-1.5 text-left hover:bg-surface-1"
            >
              {s.name && <span className="text-sm text-text-primary truncate max-w-full">{s.name}</span>}
              <span className="text-xs text-text-tertiary truncate max-w-full">{s.email}</span>
            </button>
          ))}
        </div>
        {value && (
          <button onClick={() => commit('')} className="w-full px-3 py-1.5 text-left text-sm text-text-tertiary hover:bg-surface-1 border-t border-border/60">
            {label} — effacer
          </button>
        )}
      </Popover>
    </div>
  )
}

// ── Date chip : preset periods + custom range ─────────────────────────────────
function DateChip({ value, after, before, onPreset, onCustom, labelFor }: {
  value: FilterState['date']; after: string; before: string
  onPreset: (v: FilterState['date']) => void
  onCustom: (after: string, before: string) => void
  labelFor: (v: FilterState['date']) => string
}) {
  const [open, setOpen] = useState(false)
  const [showCustom, setShowCustom] = useState(value === 'custom')
  const anchorRef = useRef<HTMLDivElement>(null)
  const presets: FilterState['date'][] = ['', '7d', '1m', '6m', '1y']
  return (
    <div className="relative flex-shrink-0" ref={anchorRef}>
      <Chip label={labelFor(value)} active={!!value} hasCaret onClick={() => { setOpen(o => !o); setShowCustom(value === 'custom') }} />
      <Popover anchorRef={anchorRef} open={open} onClose={() => setOpen(false)}>
        {!showCustom ? (
          <>
            {presets.map(p => (
              <button
                key={p || 'any'}
                onClick={() => { onPreset(p); setOpen(false) }}
                className="flex items-center gap-2 w-full px-3 py-1.5 text-left text-sm hover:bg-surface-1"
              >
                <span className="w-4 flex-shrink-0">{value === p && <Check size={15} className="text-primary" />}</span>
                {labelFor(p)}
              </button>
            ))}
            <div className="border-t border-border/60 mt-1 pt-1">
              <button onClick={() => setShowCustom(true)} className="w-full px-3 py-1.5 text-left text-sm hover:bg-surface-1">
                {labelFor('custom')}
              </button>
            </div>
          </>
        ) : (
          <div className="px-3 py-2 flex flex-col gap-2 min-w-[240px]">
            <label className="text-xs text-text-secondary flex items-center justify-between gap-2">
              Après
              <input type="date" value={after} onChange={e => onCustom(e.target.value, before)}
                     className="h-8 px-2 text-sm rounded border border-border bg-surface-0 focus:outline-none focus:border-primary" />
            </label>
            <label className="text-xs text-text-secondary flex items-center justify-between gap-2">
              Avant
              <input type="date" value={before} onChange={e => onCustom(after, e.target.value)}
                     className="h-8 px-2 text-sm rounded border border-border bg-surface-0 focus:outline-none focus:border-primary" />
            </label>
            <button onClick={() => setOpen(false)} className="self-end text-sm text-primary hover:underline">OK</button>
          </div>
        )}
      </Popover>
    </div>
  )
}

// ── The bar ───────────────────────────────────────────────────────────────────
export default function MailFolderFilterBar({ folder, onChange }: {
  folder: FilterFolder
  /** Notifies the parent of the current filter state (drafts filters client-side;
   *  the ThreadList folders apply through searchQuery here directly). */
  onChange?: (f: FilterState) => void
}) {
  const { t } = useTranslation('mail')
  const cfg = FOLDER_FILTERS[folder]
  const { searchQuery, setSearchQuery, setAdvancedSearchSeed } = useMailStore()
  const [f, setF] = useState<FilterState>(EMPTY_FILTER)

  // Reset when the folder changes (a new view starts clean).
  useEffect(() => { setF(EMPTY_FILTER) }, [folder])

  // Apply: ThreadList folders drive the shared searchQuery; drafts hand the state
  // to the parent for client-side filtering.
  const apply = useCallback((next: FilterState) => {
    setF(next)
    onChange?.(next)
    if (cfg.scope !== null) {
      setSearchQuery(isFilterActive(next) ? buildFilterQuery(cfg.scope, next) : '')
    }
  }, [cfg.scope, onChange, setSearchQuery])

  // If the query is cleared elsewhere (header search × ), drop our chips too.
  useEffect(() => {
    if (cfg.scope !== null && searchQuery === '' && isFilterActive(f)) setF(EMPTY_FILTER)
  }, [searchQuery]) // eslint-disable-line react-hooks/exhaustive-deps

  const labelForDate = (v: FilterState['date']) => ({
    '':       t('mail_filter_date_any',    { defaultValue: 'Indifférente' }),
    '7d':     t('mail_filter_date_1w',     { defaultValue: "Plus d'une semaine" }),
    '1m':     t('mail_filter_date_1m',     { defaultValue: "Plus d'un mois" }),
    '6m':     t('mail_filter_date_6m',     { defaultValue: 'Plus de six mois' }),
    '1y':     t('mail_filter_date_1y',     { defaultValue: "Plus d'un an" }),
    'custom': t('mail_filter_date_custom', { defaultValue: 'Plage personnalisée…' }),
  }[v])

  const has = (c: ChipId) => cfg.chips.includes(c)

  const openAdvanced = () => {
    // Seed the header panel with what maps cleanly onto its own fields.
    const searchIn = folder === 'all' ? 'all'
      : folder === 'sent' ? 'sent'
      : folder === 'spam' ? 'spam'
      : folder === 'drafts' ? 'drafts'
      : folder === 'starred' ? 'starred'
      : 'all'
    setAdvancedSearchSeed({
      from: f.from || undefined,
      to:   f.to || undefined,
      hasAttach: f.attach || undefined,
      searchIn,
      dateRange: f.date && f.date !== 'custom' ? f.date : undefined,
      customDate: f.date === 'custom' && f.after ? f.after : undefined,
    })
  }

  return (
    <div className="flex items-center gap-2 px-4 py-2 border-b border-[#e0e0e0] overflow-x-auto flex-shrink-0
                    [&::-webkit-scrollbar]:hidden [scrollbar-width:none]">
      {has('from') && (
        <AddressChip label={t('mail_filter_from', { defaultValue: 'De' })}
          value={f.from} onApply={v => apply({ ...f, from: v })} />
      )}
      {has('date') && (
        <DateChip value={f.date} after={f.after} before={f.before} labelFor={labelForDate}
          onPreset={v => apply({ ...f, date: v, after: '', before: '' })}
          onCustom={(a, b) => apply({ ...f, date: 'custom', after: a, before: b })} />
      )}
      {has('attach') && (
        <Chip label={t('mail_filter_attach_chip', { defaultValue: 'Contient une pièce jointe' })}
          active={f.attach} onClick={() => apply({ ...f, attach: !f.attach })} />
      )}
      {has('noAgenda') && (
        <Chip label={t('mail_filter_no_agenda', { defaultValue: 'Exclure les mises à jour d’agenda' })}
          active={f.noAgenda} onClick={() => apply({ ...f, noAgenda: !f.noAgenda })} />
      )}
      {has('to') && (
        <AddressChip label={t('mail_filter_to', { defaultValue: 'À' })}
          value={f.to} onApply={v => apply({ ...f, to: v })} />
      )}
      {has('unread') && (
        <Chip label={t('mail_filter_unread', { defaultValue: 'Non lu' })}
          active={f.unread} onClick={() => apply({ ...f, unread: !f.unread })} />
      )}
      <button
        onClick={openAdvanced}
        className="inline-flex items-center h-8 px-2 text-[13px] text-primary hover:underline whitespace-nowrap flex-shrink-0"
      >
        {t('mail_filter_advanced', { defaultValue: 'Recherche avancée' })}
      </button>
    </div>
  )
}
