import { useEffect, useMemo, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { useTranslation } from 'react-i18next'
import { Button, Checkbox, Dropdown } from '@ui'
import type { Label } from './api'

/** Nested labels are stored the way Gmail does it: "Parent/Child" names. */
export const LABEL_SEPARATOR = '/'

export interface NewLabelResult {
  /** Full name, already prefixed with the parent when nesting. */
  name:   string
  parent: string | null
}

/**
 * "New label" dialog, laid out like Gmail's: prompt + name field, then an
 * opt-in "nest under" checkbox driving a parent picker.
 */
export default function NewLabelDialog({
  labels,
  onCancel,
  onCreate,
  pending = false,
  title,
  submitLabel,
  initialName = '',
  initialParent = '',
  excludeName,
}: {
  labels:   Label[]
  onCancel: () => void
  onCreate: (result: NewLabelResult) => void
  pending?: boolean
  /** Defaults to "Nouveau libellé". */
  title?:       string
  submitLabel?: string
  initialName?: string
  /** Preset parent — used by "Add a sub-label" and when editing a nested one. */
  initialParent?: string
  /** A label can be nested under neither itself nor its own descendants. */
  excludeName?: string
}) {
  const { t } = useTranslation('mail')
  const [name,   setName]   = useState(initialName)
  const [nested, setNested] = useState(!!initialParent)
  const [parent, setParent] = useState(initialParent)
  const inputRef = useRef<HTMLInputElement>(null)

  useEffect(() => { inputRef.current?.focus() }, [])

  const parents = useMemo(
    () => labels
      .filter(l => !l.is_system)
      .filter(l => !excludeName
        || (l.name !== excludeName && !l.name.startsWith(`${excludeName}${LABEL_SEPARATOR}`)))
      .map(l => ({ value: l.name, label: l.name }))
      .sort((a, b) => a.label.localeCompare(b.label)),
    [labels, excludeName],
  )

  const trimmed   = name.trim()
  const unchanged = trimmed === initialName && (nested ? parent : '') === initialParent
  const canApply  = trimmed.length > 0 && (!nested || !!parent) && !unchanged && !pending

  function submit() {
    if (!canApply) return
    onCreate({
      name:   nested && parent ? `${parent}${LABEL_SEPARATOR}${trimmed}` : trimmed,
      parent: nested ? parent : null,
    })
  }

  // Portalled to <body>: the left sidebar establishes a containing block
  // (transform/backdrop-filter), which would otherwise shrink `fixed inset-0`
  // down to the sidebar's own width.
  return createPortal(
    <div
      className="fixed inset-0 bg-black/30 z-50 flex items-center justify-center p-4"
      onClick={onCancel}
    >
      <div
        className="bg-white rounded-lg shadow-xl w-full max-w-[460px]"
        role="dialog"
        aria-modal="true"
        aria-label={title ?? t('label_new_title', { defaultValue: 'Nouveau libellé' })}
        onClick={e => e.stopPropagation()}
        onKeyDown={e => {
          if (e.key === 'Escape') { e.stopPropagation(); onCancel() }
          if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); submit() }
        }}
      >
        <div className="px-6 pt-5 pb-2">
          <h2 className="text-[18px] font-medium text-text-primary">
            {title ?? t('label_new_title', { defaultValue: 'Nouveau libellé' })}
          </h2>
        </div>

        <div className="px-6 pb-2 space-y-4">
          <div>
            <label htmlFor="mail-new-label-name" className="block text-sm text-text-primary mb-2">
              {t('label_name_prompt', { defaultValue: 'Entrez le nom du nouveau libellé :' })}
            </label>
            <input
              id="mail-new-label-name"
              ref={inputRef}
              value={name}
              onChange={e => setName(e.target.value)}
              className="w-full h-10 border border-border rounded px-3 text-sm text-text-primary
                         focus:outline-none focus:border-primary focus:ring-1 focus:ring-primary"
            />
          </div>

          <div className="space-y-2">
            <Checkbox
              checked={nested}
              onChange={v => { setNested(v); if (!v) setParent('') }}
              label={t('label_nest_under', { defaultValue: 'Imbriquer le libellé sous :' })}
            />
            <Dropdown
              value={parent}
              // Picking a parent implies nesting — same shortcut as Gmail.
              onChange={v => { setParent(v); if (v) setNested(true) }}
              options={parents}
              placeholder={t('label_parent_placeholder', { defaultValue: 'Veuillez sélectionner un libellé parent…' })}
              width="100%"
              height={40}
              fontSize={14}
            />
          </div>
        </div>

        <div className="px-6 py-4 flex justify-end gap-2">
          <Button variant="ghost" className="min-w-[96px] justify-center" onClick={onCancel}>
            {t('common_cancel')}
          </Button>
          <Button className="min-w-[96px] justify-center" disabled={!canApply} loading={pending} onClick={submit}>
            {submitLabel ?? t('common_create')}
          </Button>
        </div>
      </div>
    </div>,
    document.body,
  )
}
