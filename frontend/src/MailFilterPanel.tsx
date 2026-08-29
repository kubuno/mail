import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { DatePicker, Dropdown, Checkbox, Button } from '@ui'
import { useMailStore } from './store'
import { mailApi } from './api'

// ── Types ─────────────────────────────────────────────────────────────────────

interface Filters {
  from:       string
  to:         string
  subject:    string
  hasWords:   string
  noWords:    string
  sizeOp:     string
  sizeValue:  string
  sizeUnit:   string
  dateRange:  string
  customDate: string | null
  searchIn:   string
  hasAttach:  boolean
}

// ── Query → fields (Gmail behaviour) ─────────────────────────────────────────
// Opening the advanced panel with a query in the bar pre-fills the fields:
// recognized operators land in their field (from:, to:, subject:, has:attachment,
// larger:/smaller:, newer_than:/after:, in:/is:), a lone `-word` goes to
// « Ne contient pas », and everything else — parenthesized groups, OR chains,
// unknown operators — is dumped verbatim into « Contient les mots », exactly as
// Gmail does with `(-label:spam OR label:trash)`.

/** Splits a query into top-level tokens, keeping quotes and (…)/{…} groups whole. */
function topLevelTokens(q: string): string[] {
  const toks: string[] = []
  let cur = '', depth = 0, quoted = false
  for (const c of q) {
    if (c === '"') { quoted = !quoted; cur += c; continue }
    if (!quoted && (c === '(' || c === '{')) depth++
    if (!quoted && (c === ')' || c === '}')) depth = Math.max(0, depth - 1)
    if (c === ' ' && !quoted && depth === 0) {
      if (cur) toks.push(cur)
      cur = ''
    } else cur += c
  }
  if (cur) toks.push(cur)
  return toks
}

/** Rebuilds the query string from the fields — the exact inverse of
 *  `queryToFilters`, so bar ⇄ fields round-trips are stable. */
export function buildQuery(f: Filters): string {
  const parts: string[] = []
  if (f.from)      parts.push(`from:${f.from.trim()}`)
  if (f.to)        parts.push(`to:${f.to.trim()}`)
  if (f.subject)   parts.push(`subject:${f.subject.trim()}`)
  if (f.hasWords)  parts.push(f.hasWords.trim())
  if (f.noWords)   parts.push(...f.noWords.trim().split(/\s+/).map(w => `-${w}`))
  if (f.hasAttach) parts.push('has:attachment')
  if (f.sizeValue) {
    const unit = { ko: 'K', mo: 'M', go: 'G' }[f.sizeUnit] ?? 'M'
    parts.push(`${f.sizeOp === 'smaller' ? 'smaller' : 'larger'}:${f.sizeValue}${unit}`)
  }
  if (f.dateRange !== '1d' || f.customDate) {
    if (f.dateRange === 'custom' && f.customDate) {
      parts.push(`after:${f.customDate.slice(0, 10).replace(/-/g, '/')}`)
    } else if (f.dateRange !== '1d') {
      parts.push(`newer_than:${f.dateRange}`)
    }
  }
  // Portée : unread/starred → opérateurs is:, sinon un dossier → in:
  if (f.searchIn === 'unread')       parts.push('is:unread')
  else if (f.searchIn === 'starred') parts.push('is:starred')
  else if (f.searchIn !== 'all')     parts.push(`in:${f.searchIn}`)
  return parts.join(' ')
}

export function queryToFilters(q: string): Partial<Filters> {
  const f: Partial<Filters> = {}
  const rest: string[] = []
  const noWords: string[] = []
  for (const tok of topLevelTokens(q.trim())) {
    const m = /^(-?)([a-z_]+):(.+)$/i.exec(tok)
    if (!m || m[1]) {
      // Bare word, group, quoted phrase or any negated token. A lone negated
      // WORD feeds « Ne contient pas »; everything else stays in the query text.
      if (/^-[^\s:(){}"]+$/.test(tok)) noWords.push(tok.slice(1))
      else if (tok) rest.push(tok)
      continue
    }
    const [, , op, value] = m
    switch (op.toLowerCase()) {
      case 'from':    if (f.from == null) f.from = value; else rest.push(tok); break
      case 'to':      if (f.to == null) f.to = value; else rest.push(tok); break
      case 'subject': if (f.subject == null) f.subject = value.replace(/^\(|\)$/g, '').replace(/^"|"$/g, ''); else rest.push(tok); break
      case 'has':
        if (value.toLowerCase() === 'attachment' || value.toLowerCase() === 'attachments') f.hasAttach = true
        else rest.push(tok)
        break
      case 'larger': case 'size': case 'smaller': {
        const sm = /^(\d+)\s*(k|ko|m|mo|g|go)?$/i.exec(value)
        if (sm && f.sizeValue == null) {
          f.sizeOp = op.toLowerCase() === 'smaller' ? 'smaller' : 'larger'
          f.sizeValue = sm[1]
          f.sizeUnit = { k: 'ko', ko: 'ko', m: 'mo', mo: 'mo', g: 'go', go: 'go' }[sm[2]?.toLowerCase() ?? 'm'] ?? 'mo'
        } else rest.push(tok)
        break
      }
      case 'newer_than':
        if (['1d', '3d', '1w', '2w', '1m', '6m', '1y'].includes(value.toLowerCase())) f.dateRange = value.toLowerCase()
        else rest.push(tok)
        break
      case 'after': {
        const d = value.replace(/\//g, '-')
        if (/^\d{4}-\d{2}-\d{2}$/.test(d)) { f.dateRange = 'custom'; f.customDate = d }
        else rest.push(tok)
        break
      }
      case 'is':
        if (value.toLowerCase() === 'unread') f.searchIn = 'unread'
        else if (value.toLowerCase() === 'starred') f.searchIn = 'starred'
        else rest.push(tok)
        break
      case 'in':
        if (['inbox', 'sent', 'drafts', 'spam', 'trash', 'archive', 'all', 'anywhere'].includes(value.toLowerCase())) {
          f.searchIn = value.toLowerCase() === 'anywhere' ? 'all' : value.toLowerCase()
        } else rest.push(tok)
        break
      default: rest.push(tok)
    }
  }
  if (noWords.length) f.noWords = noWords.join(' ')
  if (rest.length) f.hasWords = rest.join(' ')
  return f
}

const INIT: Filters = {
  from:       '',
  to:         '',
  subject:    '',
  hasWords:   '',
  noWords:    '',
  sizeOp:     'larger',
  sizeValue:  '',
  sizeUnit:   'mo',
  dateRange:  '1d',
  customDate: null,
  searchIn:   'all',
  hasAttach:  false,
}

// ── Row layout ────────────────────────────────────────────────────────────────

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="grid items-start gap-6 py-2" style={{ gridTemplateColumns: '150px 1fr' }}>
      <span className="text-sm text-text-secondary pt-1.5">{label}</span>
      <div>{children}</div>
    </div>
  )
}

function LineInput({
  value,
  onChange,
}: {
  value:    string
  onChange: (v: string) => void
}) {
  return (
    <input
      type="text"
      value={value}
      onChange={e => onChange(e.target.value)}
      className="w-full border-0 border-b border-border bg-transparent text-sm text-text-primary
                 placeholder:text-transparent focus:outline-none focus:border-primary pb-0.5
                 transition-colors"
    />
  )
}

// ── Main ──────────────────────────────────────────────────────────────────────

export default function MailFilterPanel({ onClose, initial, query, onQueryChange }: {
  onClose: () => void
  initial?: Partial<Filters>
  /** Live text of the search bar — the single source of truth the fields mirror. */
  query?: string
  /** Called with the rebuilt query whenever a field is edited (two-way sync). */
  onQueryChange?: (q: string) => void
}) {
  const { t } = useTranslation('mail')
  const { setSearchQuery } = useMailStore()
  // Fields and the bar's text are kept SYNCHRONOUS both ways (platform rule):
  // opening pre-fills from the bar's current query; typing in the bar reparses
  // into the fields; editing a field rebuilds the query into the bar. A ref
  // remembers the last query WE built so its echo doesn't reparse our state.
  // `initial` (folder filter bar chips) still wins at mount.
  const [f, setF] = useState<Filters>({ ...INIT, ...(query ? queryToFilters(query) : null), ...initial })
  const lastBuilt = useRef<string | null>(null)
  useEffect(() => {
    if (query == null || query === lastBuilt.current) return
    setF({ ...INIT, ...queryToFilters(query) })
  }, [query])

  const SIZE_OPS = [
    { value: 'larger',  label: t('mail_filter_larger') },
    { value: 'smaller', label: t('mail_filter_smaller') },
  ]

  const SIZE_UNITS = [
    { value: 'ko', label: t('mail_unit_kb') },
    { value: 'mo', label: t('mail_unit_mb') },
    { value: 'go', label: t('mail_unit_gb') },
  ]

  const DATE_RANGES = [
    { value: '1d',     label: t('mail_range_1d') },
    { value: '3d',     label: t('mail_range_3d') },
    { value: '1w',     label: t('mail_range_1w') },
    { value: '2w',     label: t('mail_range_2w') },
    { value: '1m',     label: t('mail_range_1m') },
    { value: '6m',     label: t('mail_range_6m') },
    { value: '1y',     label: t('mail_range_1y') },
    { value: 'custom', label: t('mail_range_custom') },
  ]

  const SEARCH_IN = [
    { value: 'all',     label: t('folder_all') },
    { value: 'unread',  label: t('mail_search_unread') },
    { value: 'starred', label: t('mail_search_starred') },
    { value: 'inbox',   label: t('folder_inbox') },
    { value: 'sent',    label: t('folder_sent') },
    { value: 'drafts',  label: t('folder_drafts') },
    { value: 'spam',    label: t('folder_spam') },
    { value: 'trash',   label: t('folder_trash') },
  ]

  // Field edits rebuild the query and push it to the bar right away (two-way sync).
  const set = (patch: Partial<Filters>) => setF(prev => {
    const next = { ...prev, ...patch }
    const built = buildQuery(next)
    lastBuilt.current = built
    onQueryChange?.(built)
    return next
  })

  const handleSearch = () => {
    setSearchQuery(buildQuery(f))
    onClose()
  }

  const handleReset = () => {
    setF({ ...INIT })
    setSearchQuery('')
  }

  // ── Création de filtre (règle automatique) ────────────────────────────────────
  const qc = useQueryClient()
  const [step, setStep] = useState<'conditions' | 'actions'>('conditions')
  const [act, setAct] = useState({ archive: false, markRead: false, star: false, important: false, trash: false, spam: false, labelId: '' })
  const [applyExisting, setApplyExisting] = useState(false)
  /* MÊME queryFn que les autres consommateurs de cette clé (barre latérale, page de
   * réglages) : react-query indexe par queryKey, donc deux formes de données sous la
   * même clé s'écrasent mutuellement. Le tri se fait au rendu, pas dans le queryFn. */
  const { data: labelsData } = useQuery({
    queryKey: ['mail-labels'],
    queryFn:  mailApi.listLabels,
  })
  const labels = labelsData?.labels?.filter(l => !l.is_system) ?? []
  const hasCondition = !!(f.from || f.to || f.subject || f.hasWords)
  const createFilter = async () => {
    await mailApi.createFilter({
      from_contains:    f.from    || undefined,
      to_contains:      f.to      || undefined,
      subject_contains: f.subject || undefined,
      query_contains:   f.hasWords || undefined,
      act_archive:   act.archive,
      act_mark_read: act.markRead,
      act_star:      act.star,
      act_important: act.important,
      act_trash:     act.trash,
      act_spam:      act.spam,
      act_label_id:  act.labelId || undefined,
      apply_existing: applyExisting,
    }).catch(() => {})
    qc.invalidateQueries({ queryKey: ['mail-filters'] })
    onClose()
  }

  // ── Étape « actions du filtre » ───────────────────────────────────────────────
  if (step === 'actions') {
    return (
      <div className="px-6 py-4">
        <p className="text-sm text-text-secondary mb-4">
          {t('filter_actions_intro', { defaultValue: 'Quand un message correspond, appliquer :' })}
        </p>
        <div className="space-y-3 mb-5">
          <Checkbox label={t('archive', { defaultValue: 'Archiver (ignorer la boîte de réception)' })} checked={act.archive} onChange={v => setAct(a => ({ ...a, archive: v }))} />
          <Checkbox label={t('mail_mark_read', { defaultValue: 'Marquer comme lu' })} checked={act.markRead} onChange={v => setAct(a => ({ ...a, markRead: v }))} />
          <Checkbox label={t('folder_starred', { defaultValue: 'Suivre' })} checked={act.star} onChange={v => setAct(a => ({ ...a, star: v }))} />
          <Checkbox label={t('folder_important', { defaultValue: 'Marquer comme important' })} checked={act.important} onChange={v => setAct(a => ({ ...a, important: v }))} />
          <Checkbox label={t('delete', { defaultValue: 'Supprimer (corbeille)' })} checked={act.trash} onChange={v => setAct(a => ({ ...a, trash: v }))} />
          <Checkbox label={t('spam_report', { defaultValue: 'Marquer comme spam' })} checked={act.spam} onChange={v => setAct(a => ({ ...a, spam: v }))} />
          <div className="flex items-center gap-3">
            <Checkbox label={t('filter_apply_label', { defaultValue: 'Appliquer le libellé :' })} checked={!!act.labelId} onChange={v => setAct(a => ({ ...a, labelId: v ? (labels[0]?.id ?? '') : '' }))} />
            {act.labelId && labels.length > 0 && (
              <Dropdown value={act.labelId} onChange={v => setAct(a => ({ ...a, labelId: v }))}
                options={labels.map(l => ({ value: l.id, label: l.name }))} height={32} width={180} />
            )}
          </div>
        </div>
        <div className="border-t border-border/40 pt-3 mb-4">
          <Checkbox
            label={t('filter_apply_existing', { defaultValue: 'Appliquer aussi aux conversations correspondantes déjà reçues' })}
            checked={applyExisting}
            onChange={setApplyExisting}
          />
        </div>
        <div className="flex items-center justify-end gap-3">
          <button type="button" onClick={() => setStep('conditions')}
            className="text-sm font-medium text-text-secondary hover:text-text-primary transition-colors">
            {t('common.back', { defaultValue: 'Retour' })}
          </button>
          <Button type="button" onClick={createFilter}>{t('filter_create_confirm', { defaultValue: 'Créer le filtre' })}</Button>
        </div>
      </div>
    )
  }

  return (
    <div className="px-6 py-4">
      {/* Fields */}
      <div className="divide-y divide-border/30">
        <Row label={t('mail_filter_from')}>
          <LineInput value={f.from} onChange={v => set({ from: v })} />
        </Row>
        <Row label={t('mail_filter_to')}>
          <LineInput value={f.to} onChange={v => set({ to: v })} />
        </Row>
        <Row label={t('subject')}>
          <LineInput value={f.subject} onChange={v => set({ subject: v })} />
        </Row>
        <Row label={t('mail_filter_has_words')}>
          <LineInput value={f.hasWords} onChange={v => set({ hasWords: v })} />
        </Row>
        <Row label={t('mail_filter_no_words')}>
          <LineInput value={f.noWords} onChange={v => set({ noWords: v })} />
        </Row>

        {/* Size */}
        <Row label={t('mail_filter_size')}>
          <div className="flex items-center gap-2">
            <Dropdown
              value={f.sizeOp}
              onChange={v => set({ sizeOp: v })}
              options={SIZE_OPS}
              height={32}
            />
            <input
              type="number"
              min={0}
              value={f.sizeValue}
              onChange={e => set({ sizeValue: e.target.value })}
              className="w-20 border-0 border-b border-border bg-transparent text-sm text-text-primary
                         focus:outline-none focus:border-primary pb-0.5 text-right"
            />
            <Dropdown
              value={f.sizeUnit}
              onChange={v => set({ sizeUnit: v })}
              options={SIZE_UNITS}
              height={32}
            />
          </div>
        </Row>

        {/* Date range */}
        <Row label={t('mail_filter_date_range')}>
          <div className="flex items-center gap-2">
            <Dropdown
              value={f.dateRange}
              onChange={v => set({ dateRange: v, customDate: null })}
              options={DATE_RANGES}
              height={32}
            />
            {f.dateRange === 'custom' && (
              <DatePicker
                mode="date"
                value={f.customDate}
                onChange={v => set({ customDate: v })}
                clearable
                size="sm"
                className="w-36"
              />
            )}
          </div>
        </Row>

        {/* Search in */}
        <Row label={t('mail_filter_search_in')}>
          <Dropdown
            value={f.searchIn}
            onChange={v => set({ searchIn: v })}
            options={SEARCH_IN}
            height={32}
            width="100%"
          />
        </Row>
      </div>

      {/* Attachment checkbox */}
      <div className="mt-3 mb-5">
        <Checkbox
          label={t('mail_filter_has_attachment')}
          checked={f.hasAttach}
          onChange={v => set({ hasAttach: v })}
        />
      </div>

      {/* Actions */}
      <div className="flex items-center justify-end gap-3">
        <button
          type="button"
          onClick={handleReset}
          className="text-sm font-medium text-text-secondary hover:text-text-primary transition-colors"
        >
          {t('mail_filter_reset')}
        </button>
        <button
          type="button"
          onClick={() => setStep('actions')}
          disabled={!hasCondition}
          className="text-sm font-medium text-text-secondary hover:text-text-primary disabled:opacity-40 transition-colors"
        >
          {t('mail_filter_create')}
        </button>
        <Button
          type="button"
          onClick={handleSearch}
        >
          {t('common_search')}
        </Button>
      </div>
    </div>
  )
}
