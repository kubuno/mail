import { useTranslation } from 'react-i18next'
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { Trash2, Loader2 } from 'lucide-react'
import { mailApi } from '../api'
import { BlockedSendersSection } from './BlockedSendersSection'

// ── Filters tab ───────────────────────────────────────────────────────────────

export function FiltersTab() {
  const { t } = useTranslation('mail')
  const qc = useQueryClient()
  const filtersQ = useQuery({ queryKey: ['mail-filters'], queryFn: mailApi.listFilters })
  const labelsQ  = useQuery({ queryKey: ['mail-labels'],  queryFn: mailApi.listLabels })
  const labelName = (id: string | null) => id ? (labelsQ.data?.labels.find(l => l.id === id)?.name ?? '?') : null
  const delMut = useMutation({
    mutationFn: (id: string) => mailApi.deleteFilter(id),
    onSuccess:  () => qc.invalidateQueries({ queryKey: ['mail-filters'] }),
  })
  const filters = filtersQ.data ?? []

  const condText = (f: typeof filters[number]) => {
    const parts: string[] = []
    if (f.from_contains)    parts.push(`${t('mail_filter_from', { defaultValue: 'De' })}: ${f.from_contains}`)
    if (f.to_contains)      parts.push(`${t('mail_filter_to', { defaultValue: 'À' })}: ${f.to_contains}`)
    if (f.subject_contains) parts.push(`${t('subject', { defaultValue: 'Objet' })}: ${f.subject_contains}`)
    if (f.query_contains)   parts.push(`${t('mail_filter_has_words', { defaultValue: 'Contient' })}: ${f.query_contains}`)
    return parts.join(' · ') || '—'
  }
  const actChips = (f: typeof filters[number]) => {
    const c: string[] = []
    if (f.act_archive)   c.push(t('archive', { defaultValue: 'Archiver' }))
    if (f.act_mark_read) c.push(t('mail_mark_read', { defaultValue: 'Marquer lu' }))
    if (f.act_star)      c.push(t('folder_starred', { defaultValue: 'Suivre' }))
    if (f.act_important) c.push(t('folder_important', { defaultValue: 'Important' }))
    if (f.act_trash)     c.push(t('delete', { defaultValue: 'Corbeille' }))
    if (f.act_spam)      c.push(t('spam_report', { defaultValue: 'Spam' }))
    if (f.act_label_id)  c.push(`🏷 ${labelName(f.act_label_id)}`)
    return c
  }

  return (
    <div>
      <div className="flex items-center justify-between mb-4">
        <p className="text-sm text-text-secondary">{t('mail_settings_filters_count', { count: filters.length, defaultValue: `${filters.length} filtre(s)` })}</p>
      </div>
      <p className="text-xs text-text-tertiary mb-4">
        {t('filters_hint', { defaultValue: 'Créez un filtre depuis la barre de recherche (icône filtres) → « Créer un filtre ». Les filtres s\'appliquent aux nouveaux messages reçus.' })}
      </p>
      {filtersQ.isLoading ? (
        <div className="py-10 flex justify-center"><Loader2 className="animate-spin text-text-tertiary" /></div>
      ) : filters.length === 0 ? (
        <div className="py-12 text-center text-sm text-text-tertiary">{t('filters_empty', { defaultValue: 'Aucun filtre.' })}</div>
      ) : (
        <div className="divide-y divide-border/50 border border-border rounded-lg">
          {filters.map(f => (
            <div key={f.id} className="flex items-start gap-4 px-4 py-3">
              <div className="flex-1 min-w-0">
                <div className="text-sm text-text-primary">{condText(f)}</div>
                <div className="flex flex-wrap gap-1.5 mt-1.5">
                  {actChips(f).map((c, i) => (
                    <span key={i} className="text-xs bg-surface-1 border border-border rounded-full px-2 py-0.5 text-text-secondary">{c}</span>
                  ))}
                </div>
              </div>
              <button onClick={() => delMut.mutate(f.id)} title={t('delete', { defaultValue: 'Supprimer' })}
                className="p-1.5 rounded hover:bg-danger/10 hover:text-danger text-text-tertiary transition-colors flex-shrink-0">
                <Trash2 size={15} />
              </button>
            </div>
          ))}
        </div>
      )}

      <BlockedSendersSection />
    </div>
  )
}
