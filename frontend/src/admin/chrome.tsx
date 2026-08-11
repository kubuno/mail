/** The small shared pieces of the mail admin panel: a section frame matching the
 *  core's admin cards, a verdict light, and a copy button.
 *
 *  They live here rather than in each section because a diagnostic and a key
 *  list that frame their content differently read as two unrelated pages inside
 *  one panel. */
import { useState, type ReactNode } from 'react'
import { useTranslation } from 'react-i18next'
import { Check, Copy } from 'lucide-react'
import { Button, Tooltip } from '@ui'
import type { Verdict } from './api'

/** A titled block, matching the core admin `Card`. The title is 14px bold, not
 *  small caps — the shell's rule for every section heading. */
export function Section({ icon, title, description, actions, children }: {
  icon:         ReactNode
  title:        string
  description?: string
  actions?:     ReactNode
  children:     ReactNode
}) {
  return (
    <section className="mb-4 rounded-lg border border-border bg-surface-0 p-4">
      <div className="mb-3 flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <span className="text-text-secondary">{icon}</span>
            <h2 className="text-sm font-bold text-text-primary">{title}</h2>
          </div>
          {description && (
            <p className="mt-1 max-w-3xl text-sm leading-relaxed text-text-secondary">{description}</p>
          )}
        </div>
        {actions && <div className="flex shrink-0 items-center gap-2">{actions}</div>}
      </div>
      {children}
    </section>
  )
}

/** Colours of the five verdicts. `info` and `unknown` deliberately stay neutral:
 *  they are the ones the page refuses to turn green, and a coloured dot next to
 *  "we could not check" is exactly the false reassurance to avoid. */
const VERDICT_STYLE: Record<Verdict, { dot: string; text: string; bg: string }> = {
  ok:      { dot: 'bg-success', text: 'text-success', bg: 'bg-success-light' },
  warn:    { dot: 'bg-warning', text: 'text-warning', bg: 'bg-warning-light' },
  fail:    { dot: 'bg-danger',  text: 'text-danger',  bg: 'bg-danger-light'  },
  info:    { dot: 'bg-text-tertiary', text: 'text-text-secondary', bg: 'bg-surface-2' },
  unknown: { dot: 'bg-border-strong', text: 'text-text-tertiary', bg: 'bg-surface-2' },
}

export function verdictStyle(verdict: Verdict) {
  return VERDICT_STYLE[verdict] ?? VERDICT_STYLE.unknown
}

/** The light itself, with its word — colour alone never carries the meaning. */
export function VerdictChip({ verdict }: { verdict: Verdict }) {
  const { t } = useTranslation('mail')
  const style = verdictStyle(verdict)
  const label: Record<Verdict, string> = {
    ok:      t('diag_v_ok',      { defaultValue: 'Conforme' }),
    warn:    t('diag_v_warn',    { defaultValue: 'À surveiller' }),
    fail:    t('diag_v_fail',    { defaultValue: 'À corriger' }),
    info:    t('diag_v_info',    { defaultValue: 'Publié — à votre appréciation' }),
    unknown: t('diag_v_unknown', { defaultValue: 'Non vérifiable' }),
  }
  return (
    <span className={`inline-flex items-center gap-1.5 rounded-full px-2 py-0.5 ${style.bg} ${style.text}`}
      style={{ fontSize: 'var(--kb-text-micro, 11px)' }}>
      <span className={`h-1.5 w-1.5 rounded-full ${style.dot}`} aria-hidden />
      {label[verdict] ?? label.unknown}
    </span>
  )
}

/** Copy-to-clipboard, with the confirmation on the button itself: a DNS record
 *  is copied and pasted into a zone file, and the operator needs to know the
 *  clipboard really changed before leaving the page. */
export function CopyButton({ value, label }: { value: string; label?: string }) {
  const { t } = useTranslation('mail')
  const [state, setState] = useState<'idle' | 'done' | 'error'>('idle')

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(value)
      setState('done')
    } catch {
      setState('error')
    }
    window.setTimeout(() => setState('idle'), 2000)
  }

  const tip = state === 'done'  ? t('copied',      { defaultValue: 'Copié' })
    : state === 'error' ? t('copy_failed', { defaultValue: 'Copie impossible' })
      : (label ?? t('copy', { defaultValue: 'Copier' }))

  return (
    <Tooltip label={tip}>
      <Button variant="ghost" size="sm" onClick={copy} aria-label={tip}>
        {state === 'done' ? <Check size={14} /> : <Copy size={14} />}
      </Button>
    </Tooltip>
  )
}

/** A DNS record shown in full, on one scrollable line. Never truncated with an
 *  ellipsis: a `p=` tag cut in the middle is a record that cannot be published,
 *  and the operator came here for exactly those characters. */
export function RecordBlock({ name, value, onCopyAll }: {
  name?:      string
  value:      string
  onCopyAll?: string
}) {
  return (
    <div className="flex items-start gap-2 rounded border border-border bg-surface-1 p-2">
      <div className="min-w-0 flex-1 overflow-x-auto">
        {name && (
          <div className="font-mono text-xs text-text-secondary whitespace-nowrap">{name}</div>
        )}
        <div className="font-mono text-xs text-text-primary whitespace-pre-wrap break-all select-all">
          {value}
        </div>
      </div>
      <CopyButton value={onCopyAll ?? value} />
    </div>
  )
}
