import { useEffect, useRef, useState } from 'react'
import { X, GripVertical, ListPlus, Parentheses } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { DatePicker, Dropdown, Checkbox, Button, Input, Tabs, Tooltip } from '@ui'
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
  /** Additional operator conditions as a recursive rule tree — the standard
   *  query-builder model (react-querybuilder): a group carries ONE combinator
   *  (AND/OR) and children that are conditions, raw expressions or nested
   *  groups. This represents any parenthesized mix, e.g.
   *  `(-in:spam AND in:trash) OR is:subscription`. */
  extras:     XGroup
}

export type XNode =
  | { kind: 'cond'; neg: boolean; op: string; val: string }
  | { kind: 'raw'; text: string }
  | XGroup
export interface XGroup { kind: 'group'; combinator: 'and' | 'or'; children: XNode[] }

const emptyGroup = (): XGroup => ({ kind: 'group', combinator: 'and', children: [] })

const parseCond = (sv: string) => {
  const m = /^(-?)([a-z_0-9]+):(.+)$/i.exec(sv.trim())
  return m ? { neg: !!m[1], op: m[2].toLowerCase(), val: m[3].replace(/^"|"$/g, '') } : null
}

/** One token → a node: parenthesized group (recursed), condition, or raw text. */
function tokenToNode(tok: string): XNode {
  if (tok.startsWith('(') && tok.endsWith(')')) {
    const inner = parseSeq(topLevelTokens(tok.slice(1, -1).trim()))
    return inner ?? { kind: 'raw', text: tok }
  }
  const c = parseCond(tok)
  return c ? { kind: 'cond', ...c } : { kind: 'raw', text: tok }
}

/** A token sequence → a node. Top-level `OR` splits branches; juxtaposition and
 *  the explicit `AND` keyword both mean AND. Mirrors the backend grammar. */
function parseSeq(toks: string[]): XNode | null {
  const branches: string[][] = [[]]
  for (const tk of toks) {
    if (tk.toUpperCase() === 'OR') branches.push([])
    else if (tk.toUpperCase() !== 'AND') branches[branches.length - 1].push(tk)
  }
  const nodes = branches
    .filter(b => b.length)
    .map(b => {
      const items = b.map(tokenToNode)
      return items.length === 1 ? items[0]
        : ({ kind: 'group', combinator: 'and', children: items } as XGroup)
    })
  if (!nodes.length) return null
  return nodes.length === 1 ? nodes[0] : { kind: 'group', combinator: 'or', children: nodes }
}

/** A node's query text (conditions with an empty value vanish). `wrap` adds the
 *  parentheses a multi-child group needs when embedded in a larger expression. */
function serNode(n: XNode, wrap: boolean): string {
  if (n.kind === 'raw') return n.text.trim()
  if (n.kind === 'cond') {
    const v = n.val.trim()
    if (!v) return ''
    return `${n.neg ? '-' : ''}${n.op}:${/\s/.test(v) ? `"${v}"` : v}`
  }
  const parts = n.children
    .map(c => serNode(c, c.kind === 'group' && c.children.length > 1))
    .filter(Boolean)
  if (!parts.length) return ''
  if (parts.length === 1) return parts[0]
  const joined = parts.join(n.combinator === 'or' ? ' OR ' : ' AND ')
  return wrap ? `(${joined})` : joined
}

/** Conditions in the tree (for the tab badge). */
export function countConds(n: XNode): number {
  if (n.kind === 'group') return n.children.reduce((a, c) => a + countConds(c), 0)
  if (n.kind === 'cond') return n.val.trim() ? 1 : 0
  return n.text.trim() ? 1 : 0
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
  // The rule tree: an OR root with several children needs its parentheses (it
  // is AND-combined with every other part); an AND root joins like plain parts.
  const extrasStr = serNode(f.extras, f.extras.combinator === 'or' && f.extras.children.length > 1)
  if (extrasStr) parts.push(extrasStr)
  return parts.join(' ')
}

export function queryToFilters(q: string): Partial<Filters> {
  const f: Partial<Filters> = {}
  const rest: string[] = []
  const noWords: string[] = []
  // Pre-group top-level tokens into OR chains FIRST: a token flanked by OR
  // belongs to a chain that must stay together — consuming `from:a` into the
  // "From" field out of `from:a OR from:b` would silently change the meaning.
  // The explicit AND keyword is a combinator (juxtaposition), never a word.
  const chains: string[][] = []
  let joinNext = false
  for (const tok of topLevelTokens(q.trim())) {
    if (tok.toUpperCase() === 'AND') continue
    if (tok.toUpperCase() === 'OR' && chains.length) { joinNext = true; continue }
    if (joinNext) { chains[chains.length - 1].push(tok); joinNext = false }
    else chains.push([tok])
  }
  const singles: string[] = []
  for (const chain of chains) {
    if (chain.length > 1) rest.push(chain.join(' OR '))
    else singles.push(chain[0])
  }
  for (const tok of singles) {
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
  // Split the leftovers: FREE WORDS (bare words, quoted phrases) belong to
  // « Contient les mots »; operator fragments, groups and OR chains (already
  // pre-grouped above) become editable builder rows instead.
  const isFreeWord = (u: string) =>
    !/[:(){}]/.test(u.replace(/^"|"$/g, '')) && !u.includes(' OR ') && !u.startsWith('-') && !u.startsWith('+')
  const words = rest.filter(isFreeWord)
  const extraUnits = rest.filter(u => !isFreeWord(u))
  if (words.length) f.hasWords = words.join(' ')
  if (extraUnits.length) {
    const nodes = extraUnits.map(u => parseSeq(topLevelTokens(u))).filter((n): n is XNode => n != null)
    f.extras = nodes.length === 1 && nodes[0].kind === 'group'
      ? nodes[0]
      : { kind: 'group', combinator: 'and', children: nodes }
  }
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
  extras:     emptyGroup(),
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
  // The @ui Input primitive (platform rule: primary components in search UIs) —
  // no bottom-rule underline, uniform height/typography with every other field.
  return <Input value={value} onChange={e => onChange(e.target.value)} style={{ fontSize: 14 }} />
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
  // « Additional filters » = the classic filter-builder rows (AG Grid/Airtable
  // pattern): every row is edited IN PLACE (connector, operator, value,
  // negation) and the bar's text follows live through set(). Operators with a
  // closed value set get an enumerated dropdown.
  const OP_VALUES: Record<string, string[]> = {
    in:  ['inbox', 'sent', 'drafts', 'spam', 'trash', 'archive', 'anywhere', 'snoozed'],
    is:  ['unread', 'read', 'starred', 'unstarred', 'important', 'notimportant', 'snoozed', 'muted', 'subscription'],
    has: ['attachment', 'userlabels', 'nouserlabels', 'drive', 'document', 'spreadsheet', 'presentation', 'youtube'],
    category: ['primary', 'social', 'promotions', 'updates', 'forums'],
    newer_than: ['1d', '3d', '1w', '2w', '1m', '6m', '1y'],
    older_than: ['1d', '3d', '1w', '2w', '1m', '6m', '1y'],
  }
  const X_OPS = ['label', 'is', 'in', 'has', 'category', 'list', 'filename', 'cc', 'bcc', 'deliveredto',
                 'newer_than', 'older_than', 'after', 'before', 'larger', 'smaller', 'rfc822msgid']
  // Path-based tree edits (react-querybuilder style): a node is addressed by
  // its index path from the root group; every edit clones along the path and
  // goes through set() so the bar's text follows live.
  const editTree = (mut: (root: XGroup) => void) => {
    const clone = (g: XGroup): XGroup => ({ ...g, children: g.children.map(c => (c.kind === 'group' ? clone(c) : { ...c })) })
    const root = clone(f.extras)
    mut(root)
    set({ extras: root })
  }
  const groupAt = (root: XGroup, path: number[]): XGroup => {
    let cur: XGroup = root
    for (const idx of path) {
      const child = cur.children[idx]
      if (child?.kind !== 'group') break
      cur = child
    }
    return cur
  }
  const patchNode = (path: number[], patch: Partial<XNode>) => editTree(root => {
    const parent = groupAt(root, path.slice(0, -1))
    const idx = path[path.length - 1]
    const node = parent.children[idx]
    if (node) {
      const next = { ...node, ...patch } as XNode
      // Switching to an enumerated operator whose list doesn't hold the current
      // value resets it, so the dropdown never shows a foreign value.
      if (next.kind === 'cond' && 'op' in patch && OP_VALUES[next.op] && !OP_VALUES[next.op].includes(next.val)) next.val = ''
      parent.children[idx] = next
    }
  })
  const removeNode = (path: number[]) => editTree(root => {
    const parent = groupAt(root, path.slice(0, -1))
    parent.children.splice(path[path.length - 1], 1)
  })
  const addNode = (groupPath: number[], node: XNode) => editTree(root => {
    groupAt(root, groupPath).children.push(node)
  })
  const setCombinator = (groupPath: number[], combinator: 'and' | 'or') => editTree(root => {
    groupAt(root, groupPath).combinator = combinator
  })

  // ── Drag-and-drop reordering of the tree (grip handle per node) ─────────────
  // No translucent browser ghost (project rule): the drag image is a blank 1×1,
  // the only feedback is a crisp accent insertion bar between siblings — the
  // same doctrine as the Gantt row reordering.
  const dragPathRef = useRef<number[] | null>(null)
  const [dropMark, setDropMark] = useState<{ key: string; slot?: number } | null>(null)
  const blankDragImg = useRef<HTMLImageElement | null>(null)
  useEffect(() => {
    const img = new Image()
    img.src = 'data:image/gif;base64,R0lGODlhAQABAAAAACH5BAEKAAEALAAAAAABAAEAAAICTAEAOw=='
    blankDragImg.current = img
  }, [])
  const isPrefix = (a: number[], b: number[]) => a.length <= b.length && a.every((v, i2) => v === b[i2])
  const moveNode = (from: number[], toParent: number[], toIndex: number) => {
    // A group cannot be dropped into itself or its own descendants.
    if (isPrefix(from, toParent)) return
    editTree(root => {
      const fParent = groupAt(root, from.slice(0, -1))
      const fIdx = from[from.length - 1]
      const [node] = fParent.children.splice(fIdx, 1)
      if (!node) return
      const tParent = groupAt(root, toParent)
      let ti = toIndex
      const sameParent = from.length - 1 === toParent.length && from.slice(0, -1).every((v, i2) => v === toParent[i2])
      if (sameParent && ti > fIdx) ti--
      tParent.children.splice(Math.max(0, Math.min(ti, tParent.children.length)), 0, node)
    })
  }
  const onGripDragStart = (path: number[]) => (e: React.DragEvent) => {
    dragPathRef.current = path
    e.dataTransfer.effectAllowed = 'move'
    e.dataTransfer.setData('text/plain', '')
    if (blankDragImg.current) e.dataTransfer.setDragImage(blankDragImg.current, 0, 0)
  }
  const onGripDragEnd = () => { dragPathRef.current = null; setDropMark(null) }

  // ── Recursive rendering (react-querybuilder layout) ─────────────────────────
  const renderCondRow = (n: Exclude<XNode, XGroup>, path: number[]) => (
    <div key={path.join('.')} className="flex items-center gap-2 flex-wrap">
      {n.kind === 'raw' ? (
        <Input
          value={n.text}
          onChange={e => patchNode(path, { text: e.target.value } as Partial<XNode>)}
          placeholder={t('mail_filter_expression', { defaultValue: 'expression' })}
          className="flex-1 min-w-44"
          style={{ fontSize: 14 }}
        />
      ) : (
        <>
          <Dropdown
            value={n.op}
            onChange={v => patchNode(path, { op: v } as Partial<XNode>)}
            options={X_OPS.map(o => ({ value: o, label: `${o}:` }))}
            height={36} fontSize={14} width={150} focusable
          />
          {OP_VALUES[n.op] ? (
            <Dropdown
              value={n.val}
              onChange={v => patchNode(path, { val: v } as Partial<XNode>)}
              options={OP_VALUES[n.op].map(v => ({ value: v, label: v }))}
              placeholder={t('mail_filter_extra_value', { defaultValue: 'valeur' })}
              height={36} fontSize={14} width={180} focusable
            />
          ) : (
            <Input
              value={n.val}
              onChange={e => patchNode(path, { val: e.target.value } as Partial<XNode>)}
              placeholder={t('mail_filter_extra_value', { defaultValue: 'valeur' })}
              className="w-44"
              style={{ fontSize: 14 }}
            />
          )}
          <Checkbox
            label={t('mail_filter_extra_not', { defaultValue: 'Exclure (-)' })}
            checked={n.neg}
            onChange={v => patchNode(path, { neg: v } as Partial<XNode>)}
          />
        </>
      )}
      <button
        type="button"
        onClick={() => removeNode(path)}
        title={t('mail_filter_extra_remove', { defaultValue: 'Retirer cette condition' })}
        className="p-1 rounded-full text-text-tertiary hover:text-danger hover:bg-danger/10 flex-shrink-0"
      >
        <X size={15} />
      </button>
    </div>
  )

  const renderGroup = (g: XGroup, path: number[]): React.ReactNode => (
    <div key={path.join('.') || 'root'}
      className={path.length ? 'border-l-2 border-border pl-3 py-1 space-y-2' : 'space-y-2'}>
      <div
        className={`flex items-center gap-2 flex-wrap rounded px-1 -mx-1 ${
          dropMark?.key === `grp:${path.join('.')}` ? 'bg-primary/10 outline outline-1 outline-primary' : ''
        }`}
        onDragOver={e => {
          const from = dragPathRef.current
          if (!from || isPrefix(from, path)) return
          e.preventDefault()
          e.stopPropagation()
          e.dataTransfer.dropEffect = 'move'
          const k2 = `grp:${path.join('.')}`
          setDropMark(m => (m && m.key === k2 ? m : { key: k2, before: false }))
        }}
        onDrop={e => {
          e.preventDefault()
          e.stopPropagation()
          const from = dragPathRef.current
          if (!from) return
          moveNode(from, path, g.children.length)
          dragPathRef.current = null
          setDropMark(null)
        }}
        title={dragPathRef.current ? t('mail_filter_drop_into', { defaultValue: 'Déposer dans ce groupe' }) : undefined}
      >
        <span className="text-sm text-text-secondary">
          {t('mail_filter_match', { defaultValue: 'Correspond à' })}
        </span>
        <Dropdown
          value={g.combinator}
          onChange={v => setCombinator(path, v as 'and' | 'or')}
          options={[
            { value: 'and', label: t('mail_filter_match_all', { defaultValue: 'toutes les conditions (ET)' }) },
            { value: 'or', label: t('mail_filter_match_any', { defaultValue: "l'une des conditions (OU)" }) },
          ]}
          height={36} fontSize={14} width={240} focusable
        />
        <Tooltip label={t('mail_filter_add_condition', { defaultValue: 'Ajouter une condition' })} side="top">
          <button
            type="button"
            aria-label={t('mail_filter_add_condition', { defaultValue: 'Ajouter une condition' })}
            onClick={() => addNode(path, { kind: 'cond', neg: false, op: 'label', val: '' })}
            className="p-1.5 rounded text-text-secondary hover:text-text-primary hover:bg-surface-2 flex-shrink-0"
          >
            <ListPlus size={16} />
          </button>
        </Tooltip>
        <Tooltip label={t('mail_filter_add_group', { defaultValue: 'Ajouter un groupe' })} side="top">
          <button
            type="button"
            aria-label={t('mail_filter_add_group', { defaultValue: 'Ajouter un groupe' })}
            onClick={() => addNode(path, { kind: 'group', combinator: g.combinator === 'or' ? 'and' : 'or', children: [{ kind: 'cond', neg: false, op: 'label', val: '' }] })}
            className="p-1.5 rounded text-text-secondary hover:text-text-primary hover:bg-surface-2 flex-shrink-0"
          >
            <Parentheses size={16} />
          </button>
        </Tooltip>
        {path.length > 0 && (
          <button
            type="button"
            onClick={() => removeNode(path)}
            title={t('mail_filter_group_remove', { defaultValue: 'Retirer ce groupe' })}
            className="p-1 rounded-full text-text-tertiary hover:text-danger hover:bg-danger/10 flex-shrink-0"
          >
            <X size={15} />
          </button>
        )}
      </div>
      {g.children.map((c, i) => {
        const childPath = [...path, i]
        const key = childPath.join('.')
        return (
          <div
            key={key}
            className="relative"
            onDragOver={e => {
              if (!dragPathRef.current) return
              e.preventDefault()
              e.stopPropagation()
              e.dataTransfer.dropEffect = 'move'
              const r2 = e.currentTarget.getBoundingClientRect()
              // ONE boundary per sibling pair: "after row N" and "before row
              // N+1" are the same insertion SLOT — hovering N's bottom half or
              // N+1's top half marks the exact same bar. Only update when the
              // slot actually changes (a write per dragover event re-renders in
              // a loop and makes the whole panel shiver).
              const slot = e.clientY < r2.top + r2.height / 2 ? i : i + 1
              const gkey = path.join('.') || 'root'
              setDropMark(m => (m && m.key === gkey && m.slot === slot ? m : { key: gkey, slot }))
            }}
            onDrop={e => {
              e.preventDefault()
              e.stopPropagation()
              const from = dragPathRef.current
              if (!from) return
              const gkey = path.join('.') || 'root'
              const slot = dropMark?.key === gkey && dropMark.slot != null ? dropMark.slot : i
              moveNode(from, path, slot)
              dragPathRef.current = null
              setDropMark(null)
            }}
          >
            {/* Insertion bar as an OVERLAY: absolutely positioned so it never
                shifts the layout under the pointer. Rendered once per slot: at
                the top of the slot's child, or under the last child for the
                final slot. No dragleave-clear — the mark simply moves on. */}
            {dropMark?.key === (path.join('.') || 'root') && dropMark.slot === i && (
              <div className="absolute -top-px left-0 right-0 h-0.5 bg-primary rounded pointer-events-none" />
            )}
            <div className="flex items-start gap-1">
              <button
                type="button"
                draggable
                onDragStart={onGripDragStart(childPath)}
                onDragEnd={onGripDragEnd}
                title={t('mail_filter_reorder', { defaultValue: 'Réordonner' })}
                className="cursor-grab p-1 mt-1.5 rounded text-text-tertiary hover:text-text-primary hover:bg-surface-2 flex-shrink-0"
              >
                <GripVertical size={14} />
              </button>
              <div className="flex-1 min-w-0">
                {c.kind === 'group' ? renderGroup(c, childPath) : renderCondRow(c, childPath)}
              </div>
            </div>
            {i === g.children.length - 1 &&
              dropMark?.key === (path.join('.') || 'root') && dropMark.slot === i + 1 && (
              <div className="absolute -bottom-px left-0 right-0 h-0.5 bg-primary rounded pointer-events-none" />
            )}
          </div>
        )
      })}
    </div>
  )

  // The panel's two facets: the classic criteria fields, and the dynamic
  // operator filters — kept in separate tabs (user request).
  const [tab, setTab] = useState<'criteria' | 'extras'>('criteria')

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
                options={labels.map(l => ({ value: l.id, label: l.name }))} height={36} width={180} />
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
      <Tabs
        tabs={[
          { id: 'criteria', label: t('mail_filter_tab_criteria', { defaultValue: 'Critères' }) },
          { id: 'extras', label: t('mail_filter_extras', { defaultValue: 'Filtres supplémentaires' }),
            badge: countConds(f.extras) || undefined },
        ]}
        value={tab}
        onChange={v => setTab(v)}
        size="sm"
        className="mb-3"
        t={t}
      />

      {/* Fields */}
      <div className={tab === 'criteria' ? undefined : 'hidden'}>
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
              height={36}
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
              height={36}
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
              height={36}
            />
            {f.dateRange === 'custom' && (
              <DatePicker
                mode="date"
                value={f.customDate}
                onChange={v => set({ customDate: v })}
                clearable
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
            height={36}
            width="100%"
          />
        </Row>
      </div>

      {/* Attachment checkbox */}
      <div className={tab === 'criteria' ? 'mt-3 mb-5' : 'hidden'}>
        <Checkbox
          label={t('mail_filter_has_attachment')}
          checked={f.hasAttach}
          onChange={v => set({ hasAttach: v })}
        />
      </div>

      {/* ── Additional operator filters tab ──────────────────────────────────
          The standard recursive query-builder (react-querybuilder model): each
          group has ONE combinator (all/any) and holds conditions, raw
          expressions and nested groups — representing any parenthesized AND/OR
          mix, e.g. `(-in:spam AND in:trash) OR is:subscription`. Every edit
          rewrites the bar's query live. */}
      <div className={tab === 'extras' ? 'mb-5' : 'hidden'}>
        {renderGroup(f.extras, [])}
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
