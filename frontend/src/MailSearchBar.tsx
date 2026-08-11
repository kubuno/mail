/**
 * Custom search bar for the mail module (replaces the core SearchBar via
 * SearchConfig.SearchComponent). Gmail-like experience:
 *  - inline ghost completion of operators with a "Tab" hint chip
 *  - quick-filter chips (has attachment / last 7 days / from me)
 *  - operator suggestions ("in:se" → "in:sent – Sent messages")
 *  - live thread preview while typing, full search on Enter
 *  - the tune button opens the existing advanced-filter panel
 */
import { useState, useRef, useEffect, useMemo } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate, useLocation } from 'react-router-dom'
import { useQuery } from '@tanstack/react-query'
import { Search, X, Mic, Mail as MailIcon, Paperclip } from 'lucide-react'
import { useVoiceDictation } from '@kubuno/sdk'
import { mailApi, Thread } from './api'
import { useMailStore } from './store'
import MailFilterPanel from './MailFilterPanel'

function TuneIcon({ size = 20 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none"
      stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
      <line x1="4" y1="6"  x2="20" y2="6" />
      <line x1="4" y1="12" x2="20" y2="12" />
      <line x1="4" y1="18" x2="20" y2="18" />
      <circle cx="8"  cy="6"  r="2" fill="white" />
      <circle cx="16" cy="12" r="2" fill="white" />
      <circle cx="10" cy="18" r="2" fill="white" />
    </svg>
  )
}

interface Sg {
  ins:  string   // text inserted in the query, e.g. "in:sent" or "from:"
  desc: string   // human description shown next to it
}

function fmtDate(s: string, lang: string) {
  const d = new Date(s)
  const now = new Date()
  if (d.toDateString() === now.toDateString()) {
    return d.toLocaleTimeString(lang, { hour: '2-digit', minute: '2-digit' })
  }
  if (d.getFullYear() === now.getFullYear()) {
    return d.toLocaleDateString(lang, { month: 'short', day: 'numeric' })
  }
  return d.toLocaleDateString(lang, { month: 'short', day: 'numeric', year: 'numeric' })
}

/** Last whitespace-separated token of the query (ignoring a leading '-'). */
function lastToken(q: string): { prefix: string; token: string; neg: string } {
  const m = /(^|\s)(\S*)$/.exec(q)
  const raw = m?.[2] ?? ''
  const prefix = q.slice(0, q.length - raw.length)
  const neg = raw.startsWith('-') ? '-' : ''
  return { prefix, token: neg ? raw.slice(1) : raw, neg }
}

export default function MailSearchBar() {
  const { t, i18n } = useTranslation('mail')
  const navigate = useNavigate()
  const { pathname } = useLocation()
  const { searchQuery, setSearchQuery, setSelectedThread, advancedSearchSeed, setAdvancedSearchSeed, newFilterNonce } = useMailStore()

  const [q, setQ]                 = useState(searchQuery)
  const [open, setOpen]           = useState(false)
  const [filterOpen, setFilterOpen] = useState(false)

  // A folder filter bar asked to open the advanced panel, seeded with its chips.
  useEffect(() => {
    if (advancedSearchSeed) { setOpen(false); setFilterOpen(true) }
  }, [advancedSearchSeed])

  // The shell's New menu (« Nouveau filtre / règle ») bumps a store nonce we
  // watch to open the advanced-filter panel, which hosts the rule-creation flow.
  const prevFilterNonce = useRef(newFilterNonce)
  useEffect(() => {
    if (newFilterNonce !== prevFilterNonce.current) {
      prevFilterNonce.current = newFilterNonce
      setOpen(false); setFilterOpen(true)
    }
  }, [newFilterNonce])
  const [focused, setFocused]     = useState(false)
  const [hi, setHi]               = useState(-1)
  const [debQ, setDebQ]           = useState('')
  const containerRef = useRef<HTMLDivElement>(null)
  const inputRef     = useRef<HTMLInputElement>(null)
  const focusedRef   = useRef(false)
  focusedRef.current = focused

  // Reflect external query changes (filter panel, "filter similar" action…)
  // without clobbering what the user is currently typing.
  useEffect(() => { if (!focusedRef.current) setQ(searchQuery) }, [searchQuery])

  // Debounced preview query.
  useEffect(() => {
    const id = setTimeout(() => setDebQ(q.trim()), 250)
    return () => clearTimeout(id)
  }, [q])

  const { data: labelData } = useQuery({
    queryKey: ['mail-labels'],
    queryFn:  () => mailApi.listLabels(),
    enabled:  open,
    staleTime: 60_000,
  })

  const { data: preview } = useQuery({
    queryKey: ['mail-search-preview', debQ],
    queryFn:  () => mailApi.listThreads({ search: debQ, limit: 5 }),
    enabled:  open && !!debQ,
    staleTime: 30_000,
  })
  const results: Thread[] = (open && debQ ? preview?.threads : undefined) ?? []

  // ── Operator catalog ────────────────────────────────────────────────────────
  const catalog = useMemo<Sg[]>(() => {
    const c: Sg[] = [
      { ins: 'in:inbox',            desc: t('folder_inbox') },
      { ins: 'in:sent',             desc: t('srch_in_sent') },
      { ins: 'in:drafts',           desc: t('folder_drafts') },
      { ins: 'in:spam',             desc: t('folder_spam') },
      { ins: 'in:trash',            desc: t('folder_trash') },
      { ins: 'in:archive',          desc: t('srch_in_archive') },
      { ins: 'in:anywhere',         desc: t('srch_in_anywhere') },
      { ins: 'in:snoozed',          desc: t('folder_snoozed') },
      { ins: 'is:unread',           desc: t('mail_search_unread') },
      { ins: 'is:read',             desc: t('srch_is_read') },
      { ins: 'is:starred',          desc: t('mail_search_starred') },
      { ins: 'is:important',        desc: t('srch_is_important') },
      { ins: 'is:snoozed',          desc: t('folder_snoozed') },
      { ins: 'is:muted',            desc: t('srch_is_muted') },
      { ins: 'has:attachment',      desc: t('mail_filter_has_attachment') },
      { ins: 'has:userlabels',      desc: t('srch_has_userlabels') },
      { ins: 'has:nouserlabels',    desc: t('srch_has_nouserlabels') },
      { ins: 'has:drive',           desc: t('srch_has_drive') },
      { ins: 'has:document',        desc: t('srch_has_document') },
      { ins: 'has:spreadsheet',     desc: t('srch_has_spreadsheet') },
      { ins: 'has:presentation',    desc: t('srch_has_presentation') },
      { ins: 'has:youtube',         desc: t('srch_has_youtube') },
      { ins: 'category:primary',       desc: t('mail_tab_primary') },
      { ins: 'category:promotions',    desc: t('mail_tab_promotions') },
      { ins: 'category:social',        desc: t('mail_tab_social') },
      { ins: 'category:notifications', desc: t('mail_tab_notifications') },
      { ins: 'from:',        desc: t('srch_op_from') },
      { ins: 'to:',          desc: t('srch_op_to') },
      { ins: 'cc:',          desc: t('srch_op_cc') },
      { ins: 'bcc:',         desc: t('srch_op_bcc') },
      { ins: 'subject:',     desc: t('srch_op_subject') },
      { ins: 'label:',       desc: t('srch_op_label') },
      { ins: 'filename:',    desc: t('srch_op_filename') },
      { ins: 'list:',        desc: t('srch_op_list') },
      { ins: 'deliveredto:', desc: t('srch_op_deliveredto') },
      { ins: 'rfc822msgid:', desc: t('srch_op_msgid') },
      { ins: 'after:',       desc: t('srch_op_after') },
      { ins: 'before:',      desc: t('srch_op_before') },
      { ins: 'older_than:',  desc: t('srch_op_older_than') },
      { ins: 'newer_than:',  desc: t('srch_op_newer_than') },
      { ins: 'larger:',      desc: t('srch_op_larger') },
      { ins: 'smaller:',     desc: t('srch_op_smaller') },
    ]
    for (const l of labelData?.labels ?? []) {
      if (l.is_system) continue
      c.push({ ins: `label:${l.name.toLowerCase().replace(/\s+/g, '-')}`, desc: `${t('srch_op_label')} « ${l.name} »` })
    }
    return c
  }, [t, labelData])

  const { prefix, token, neg } = lastToken(q)
  const suggestions = useMemo<Sg[]>(() => {
    if (!token) return []
    const lower = token.toLowerCase()
    return catalog
      .filter(s => s.ins.startsWith(lower) && s.ins !== lower)
      .slice(0, 4)
  }, [catalog, token])

  // Ghost inline completion = remainder of the best suggestion.
  const ghost = suggestions.length > 0 ? suggestions[0].ins.slice(token.length) : ''

  const commit = (query: string) => {
    setSearchQuery(query.trim())
    setOpen(false)
    setHi(-1)
  }

  const acceptSuggestion = (s: Sg) => {
    const next = `${prefix}${neg}${s.ins}${s.ins.endsWith(':') ? '' : ' '}`
    setQ(next)
    setHi(-1)
    inputRef.current?.focus()
  }

  const openThread = (th: Thread) => {
    commit(q)
    if (pathname.startsWith('/mail/settings')) {
      // The settings route has no thread reader: go back to the mailbox first.
      // The route-change effect in MailApp resets the selected thread, so the
      // selection is applied on the next macrotask, after that reset ran.
      navigate('/mail')
      setTimeout(() => useMailStore.getState().setSelectedThread(th.id), 0)
    } else {
      setSelectedThread(th.id)
    }
  }

  const handleChange = (value: string) => {
    setQ(value)
    setHi(-1)
    setFilterOpen(false)
    setOpen(true)
    if (value === '') setSearchQuery('')
  }

  const voice = useVoiceDictation({ getSeed: () => '', onText: handleChange })

  // Escape closes the dropdown and the filter panel. Document-level listener:
  // opening the panel re-renders the bar and drops the focus onto <body>, so
  // neither the input nor the container would receive the key event.
  useEffect(() => {
    if (!open && !filterOpen) return
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        setOpen(false)
        setFilterOpen(false)
        setHi(-1)
      }
    }
    document.addEventListener('keydown', handler)
    return () => document.removeEventListener('keydown', handler)
  }, [open, filterOpen])

  // Close on outside click.
  useEffect(() => {
    if (!open && !filterOpen && !voice.listening) return
    const handler = (e: MouseEvent) => {
      if (containerRef.current && !containerRef.current.contains(e.target as Node)) {
        setOpen(false)
        setFilterOpen(false)
        setHi(-1)
        voice.stop()
      }
    }
    document.addEventListener('mousedown', handler)
    return () => document.removeEventListener('mousedown', handler)
  }, [open, filterOpen, voice.listening]) // eslint-disable-line react-hooks/exhaustive-deps

  // ── Quick-filter chips ──────────────────────────────────────────────────────
  const CHIPS = [
    { id: 'attach', token: 'has:attachment', label: t('srch_chip_attachment') },
    { id: '7days',  token: 'newer_than:7d',  label: t('srch_chip_7days') },
    { id: 'fromme', token: 'from:me',        label: t('srch_chip_fromme') },
  ]
  const toggleChip = (tok: string) => {
    const has = q.split(/\s+/).includes(tok)
    const next = has
      ? q.split(/\s+/).filter(w => w !== tok).join(' ')
      : (q.trim() ? `${q.trim()} ${tok}` : tok)
    setQ(next + (next && !has ? ' ' : ''))
    inputRef.current?.focus()
  }

  // ── Keyboard ────────────────────────────────────────────────────────────────
  const navLength = suggestions.length + results.length + (debQ ? 1 : 0)
  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Tab' && ghost) {
      e.preventDefault()
      acceptSuggestion(suggestions[0])
      return
    }
    if (e.key === 'Escape') {
      setOpen(false)
      setFilterOpen(false)
      setHi(-1)
      return
    }
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      if (!navLength) return
      e.preventDefault()
      setHi(h => {
        const d = e.key === 'ArrowDown' ? 1 : -1
        const n = h + d
        return n < -1 ? navLength - 1 : n >= navLength ? -1 : n
      })
      return
    }
    if (e.key === 'Enter') {
      e.preventDefault()
      if (hi >= 0 && hi < suggestions.length) {
        acceptSuggestion(suggestions[hi])
      } else if (hi >= suggestions.length && hi < suggestions.length + results.length) {
        openThread(results[hi - suggestions.length])
      } else {
        commit(q)
      }
    }
  }

  // The dropdown (chips/suggestions/results) only appears once something is
  // typed; a focused empty field stays a plain pill.
  const expanded = (open && q.trim() !== '') || filterOpen
  const isActive = focused || open || filterOpen

  // ── Search row (shared between collapsed pill and expanded panel) ───────────
  const searchRow = (
    <div className="flex items-center h-12 flex-shrink-0">
      <div className="pl-4 pr-2 flex-shrink-0">
        <Search size={20} className="text-text-secondary" />
      </div>

      <div className="relative flex-1 min-w-0">
        <input
          ref={inputRef}
          type="text"
          value={q}
          placeholder={t('mail_search_ph')}
          onChange={e => handleChange(e.target.value)}
          onKeyDown={onKeyDown}
          onFocus={() => { setFocused(true); setFilterOpen(false); setOpen(true) }}
          onBlur={() => setFocused(false)}
          autoComplete="off"
          spellCheck={false}
          // `block` kills the inline baseline strut: the wrapper is then exactly
          // the input's height, so the ghost overlay centers on the same pixel.
          className="block w-full bg-transparent outline-none min-w-0 text-text-primary placeholder:text-text-tertiary"
        />
        {/* Ghost completion + Tab hint, aligned after the typed text. */}
        {focused && ghost && (
          <div aria-hidden className="absolute inset-y-0 left-0 flex items-center pointer-events-none overflow-hidden whitespace-pre">
            <span className="invisible">{q}</span>
            <span className="text-text-tertiary">{ghost}</span>
            <span className="ml-2 text-[11px] leading-none text-text-secondary border border-border rounded px-1.5 py-0.5 flex-shrink-0">
              {t('srch_tab')}
            </span>
          </div>
        )}
      </div>

      {q && (
        <button
          onMouseDown={e => { e.preventDefault(); setQ(''); setSearchQuery(''); inputRef.current?.focus() }}
          className="flex-shrink-0 px-1 text-text-tertiary hover:text-text-primary"
          aria-label={t('srch_clear')}
        >
          <X size={16} />
        </button>
      )}

      <div className="flex items-center flex-shrink-0 pr-2">
        {voice.enabled && (
          <button
            onClick={voice.toggleVoice}
            aria-label={t('srch_voice')}
            title={t('srch_voice')}
            className={`w-10 h-10 flex items-center justify-center rounded-full transition-colors
              ${(voice.listening || voice.voiceLoading)
                ? 'text-red-500 bg-red-500/10'
                : 'text-text-secondary hover:bg-[#e8f0fe]'}`}
          >
            <Mic size={20} className={voice.listening ? 'animate-pulse' : ''} />
          </button>
        )}
        <div className="w-px h-6 mx-2 flex-shrink-0 bg-border" />
        <button
          onClick={() => { setFilterOpen(v => !v); setOpen(false); setHi(-1) }}
          aria-label={t('srch_options')}
          className={`w-10 h-10 flex items-center justify-center rounded-full transition-colors
            ${filterOpen ? 'bg-primary-light text-primary' : 'text-text-secondary hover:bg-[#e8f0fe]'}`}
        >
          {filterOpen ? <X size={20} /> : <TuneIcon size={20} />}
        </button>
      </div>
    </div>
  )

  // Bold the typed part of a suggestion, like Gmail.
  const renderIns = (ins: string) => (
    <span className="text-xs text-text-primary">
      <span className="font-semibold">{ins.slice(0, token.length)}</span>
      {ins.slice(token.length)}
    </span>
  )

  const rowBase = 'w-full flex items-center gap-3 px-4 py-2 text-left cursor-pointer'
  const dropdown = (
    <>
      {/* Quick-filter chips */}
      <div className="flex items-center gap-2 px-4 py-3 border-b border-border/60 overflow-x-auto">
        {CHIPS.map(ch => {
          const active = q.split(/\s+/).includes(ch.token)
          return (
            <button
              key={ch.id}
              onMouseDown={e => { e.preventDefault(); toggleChip(ch.token) }}
              className={`flex-shrink-0 text-xs rounded-lg border px-3.5 py-1.5 transition-colors
                ${active
                  ? 'bg-primary-light border-primary/40 text-primary'
                  : 'border-border text-text-secondary hover:bg-surface-1'}`}
            >
              {ch.label}
            </button>
          )
        })}
      </div>

      {/* Operator suggestions */}
      {suggestions.length > 0 && (
        <div className="py-1 border-b border-border/60">
          {suggestions.map((s, i) => (
            <div
              key={s.ins}
              onMouseDown={e => { e.preventDefault(); acceptSuggestion(s) }}
              onMouseEnter={() => setHi(i)}
              className={`${rowBase} ${hi === i ? 'bg-surface-2' : 'hover:bg-surface-1'}`}
            >
              <Search size={17} className="text-text-tertiary flex-shrink-0" />
              {renderIns(s.ins)}
              <span className="text-xs text-text-secondary truncate">— {s.desc}</span>
            </div>
          ))}
        </div>
      )}

      {/* Live results preview */}
      {results.length > 0 && (
        <div className="py-1 border-b border-border/60">
          {results.map((th, i) => {
            const idx = suggestions.length + i
            return (
              <div
                key={th.id}
                onMouseDown={e => { e.preventDefault(); openThread(th) }}
                onMouseEnter={() => setHi(idx)}
                className={`${rowBase} ${hi === idx ? 'bg-surface-2' : 'hover:bg-surface-1'}`}
              >
                <MailIcon size={18} className="text-text-secondary flex-shrink-0" />
                <div className="flex-1 min-w-0">
                  <div className={`text-sm truncate ${th.unread_count > 0 ? 'font-semibold text-text-primary' : 'text-text-primary'}`}>
                    {th.subject || t('mail_no_subject')}
                  </div>
                  <div className="text-xs text-text-secondary truncate">
                    {th.last_sender_name || th.last_sender_email}
                  </div>
                </div>
                <div className="flex items-center gap-1.5 flex-shrink-0 text-text-secondary">
                  {th.has_attachments && <Paperclip size={14} />}
                  <span className="text-xs">{fmtDate(th.last_message_at, i18n.language)}</span>
                </div>
              </div>
            )
          })}
        </div>
      )}

      {/* Footer: run the full search */}
      {!!debQ && (
        <div
          onMouseDown={e => { e.preventDefault(); commit(q) }}
          onMouseEnter={() => setHi(navLength - 1)}
          className={`${rowBase} justify-between py-3 ${hi === navLength - 1 ? 'bg-surface-2' : 'hover:bg-surface-1'}`}
        >
          <div className="flex items-center gap-3 min-w-0">
            <Search size={17} className="text-text-tertiary flex-shrink-0" />
            <span className="text-xs text-text-primary truncate">{t('srch_all_results', { q: debQ })}</span>
          </div>
          <span className="text-xs text-text-tertiary flex-shrink-0 ml-3">{t('srch_press_enter')}</span>
        </div>
      )}
    </>
  )

  return (
    <div ref={containerRef} className="relative w-full">
      {voice.voiceToast}

      {/* Invisible spacer keeps the header height while the panel floats. */}
      {expanded && <div key="spacer" className="h-12" aria-hidden />}

      {/*
       * ONE stable keyed node for both states (pill / expanded panel): only its
       * classes and styles change. Splitting into two JSX branches would
       * remount the <input> on the first keystroke and drop the focus.
       */}
      <div
        key="bar"
        className={expanded ? 'absolute left-0 right-0 top-0 z-[70]' : 'transition-all'}
        style={expanded ? {
          background:   '#ffffff',
          border:       '1px solid #e0e0e0',
          borderRadius: 24,
          boxShadow:    '0 4px 20px rgba(0,0,0,0.16)',
          overflow:     'hidden',
        } : {
          background:   isActive ? '#ffffff' : '#eaeef5',
          boxShadow:    isActive ? '0 1px 3px rgba(0,0,0,0.2), 0 2px 6px rgba(0,0,0,0.1)' : 'none',
          border:       `1px solid ${isActive ? '#e0e0e0' : 'transparent'}`,
          borderRadius: '9999px',
        }}
      >
        {searchRow}
        {expanded && (filterOpen ? (
          <>
            <div style={{ height: 1, background: 'var(--color-border)', margin: '0 16px' }} />
            <MailFilterPanel
              key={advancedSearchSeed ? 'seeded' : 'blank'}
              initial={advancedSearchSeed ?? undefined}
              onClose={() => { setFilterOpen(false); setAdvancedSearchSeed(null) }}
            />
          </>
        ) : (
          dropdown
        ))}
      </div>
    </div>
  )
}
