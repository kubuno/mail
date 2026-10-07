import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { Plus, Trash2, Loader2, Tag } from 'lucide-react'
import { mailApi } from '../api'
import { Button, Dropdown } from '@ui'

// ── Labels tab ────────────────────────────────────────────────────────────────

const LABEL_COLORS = [
  '#1a73e8', '#e8711a', '#0f9d58', '#d93025',
  '#9c27b0', '#f9ab00', '#00838f', '#6d4c41',
]

export function LabelsTab() {
  const { t } = useTranslation('mail')
  const qc = useQueryClient()
  const [newName,    setNewName]    = useState('')
  const [newColor,   setNewColor]   = useState(LABEL_COLORS[0])
  const [showCreate, setShowCreate] = useState(false)
  const [accountId,  setAccountId]  = useState<string | null>(null)

  const accountsQ = useQuery({ queryKey: ['mail-accounts'], queryFn: mailApi.listAccounts })
  const labelsQ   = useQuery({ queryKey: ['mail-labels'],   queryFn: mailApi.listLabels   })

  const accounts = accountsQ.data?.accounts ?? []
  const labels   = labelsQ.data?.labels     ?? []

  const selectedAccountId = accountId ?? accounts[0]?.id ?? null

  const createMut = useMutation({
    mutationFn: () => mailApi.createLabel({
      account_id: selectedAccountId!,
      name:       newName.trim(),
      color:      newColor,
    }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['mail-labels'] })
      setNewName('')
      setShowCreate(false)
    },
  })

  const deleteMut = useMutation({
    mutationFn: (id: string) => mailApi.deleteLabel(id),
    onSuccess:  () => qc.invalidateQueries({ queryKey: ['mail-labels'] }),
  })

  if (labelsQ.isLoading) {
    return (
      <div className="flex justify-center py-8">
        <Loader2 size={20} className="animate-spin text-text-tertiary" />
      </div>
    )
  }

  return (
    <div>
      <div className="flex items-center justify-between mb-5">
        <p className="text-sm text-text-tertiary">
          {t('mail_settings_labels_count', { count: labels.length })}
        </p>
        {accounts.length > 0 && (
          <button
            onClick={() => setShowCreate(true)}
            className="flex items-center gap-1.5 text-sm text-primary hover:underline"
          >
            <Plus size={14} />
            {t('mail_settings_new_label')}
          </button>
        )}
      </div>

      {labels.length === 0 && !showCreate ? (
        <div className="text-center py-12">
          <Tag size={32} className="opacity-30 mx-auto mb-3 text-text-tertiary" />
          <p className="text-sm text-text-tertiary">{t('mail_settings_no_labels')}</p>
        </div>
      ) : (
        <div>
          {labels.map(label => (
            <div
              key={label.id}
              className="flex items-center justify-between py-2.5 border-b border-[#e8eaed] last:border-0"
            >
              <div className="flex items-center gap-3">
                <span
                  className="w-3 h-3 rounded-full flex-shrink-0"
                  style={{ background: label.color ?? '#5f6368' }}
                />
                <span className="text-sm text-text-primary">{label.name}</span>
                {label.is_system && (
                  <span className="text-xs text-text-tertiary bg-surface-2 px-1.5 py-0.5 rounded">
                    {t('mail_settings_label_system')}
                  </span>
                )}
              </div>
              {!label.is_system && (
                <button
                  onClick={() => deleteMut.mutate(label.id)}
                  className="p-1 rounded text-text-tertiary hover:text-danger hover:bg-danger/10"
                >
                  <Trash2 size={13} />
                </button>
              )}
            </div>
          ))}
        </div>
      )}

      {showCreate && (
        <div className="mt-4 p-4 border border-border rounded-xl bg-surface-1">
          <h3 className="text-sm font-medium text-text-primary mb-3">{t('mail_settings_new_label')}</h3>
          <div className="space-y-3">
            {accounts.length > 1 && (
              <div>
                <label className="block text-xs font-medium text-text-secondary mb-1">{t('mail_settings_account')}</label>
                <Dropdown
                  className="w-full"
                  value={selectedAccountId ?? ''}
                  onChange={v => setAccountId(v)}
                  options={accounts.map(a => ({ value: a.id, label: a.name }))}
                />
              </div>
            )}
            <div>
              <label className="block text-xs text-text-secondary mb-1">{t('mail_settings_name')}</label>
              <input
                type="text"
                value={newName}
                onChange={e => setNewName(e.target.value)}
                placeholder={t('mail_settings_label_name_placeholder')}
                autoFocus
                className="w-full border border-border rounded-lg px-3 py-2 text-sm
                           focus:outline-none focus:ring-2 focus:ring-primary/30"
              />
            </div>
            <div>
              <label className="block text-xs text-text-secondary mb-1">{t('mail_settings_color')}</label>
              <div className="flex gap-2">
                {LABEL_COLORS.map(c => (
                  <button
                    key={c}
                    onClick={() => setNewColor(c)}
                    className={`w-6 h-6 rounded-full transition-transform ${
                      newColor === c
                        ? 'scale-125 ring-2 ring-offset-1 ring-gray-400'
                        : 'hover:scale-110'
                    }`}
                    style={{ background: c }}
                  />
                ))}
              </div>
            </div>
            <div className="flex justify-end gap-2">
              <Button
                variant="ghost"
                size="sm"
                onClick={() => { setShowCreate(false); setNewName('') }}
              >
                {t('common_cancel')}
              </Button>
              <Button
                size="sm"
                onClick={() => createMut.mutate()}
                disabled={!newName.trim() || !selectedAccountId}
                loading={createMut.isPending}
              >
                {t('common_create')}
              </Button>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}
