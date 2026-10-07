import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { Loader2, Ban } from 'lucide-react'
import { mailApi } from '../api'
import { Button } from '@ui'

// ── Blocked addresses ─────────────────────────────────────────────────────────

export function BlockedSendersSection() {
  const { t } = useTranslation('mail')
  const qc = useQueryClient()
  const [email, setEmail] = useState('')
  const blockedQ = useQuery({ queryKey: ['mail-blocked'], queryFn: mailApi.listBlocked })
  const blocked = blockedQ.data ?? []

  const addMut = useMutation({
    mutationFn: () => mailApi.blockSender(email.trim()),
    onSuccess:  () => { setEmail(''); qc.invalidateQueries({ queryKey: ['mail-blocked'] }) },
  })
  const delMut = useMutation({
    mutationFn: (id: string) => mailApi.unblockSender(id),
    onSuccess:  () => qc.invalidateQueries({ queryKey: ['mail-blocked'] }),
  })

  const canAdd = email.trim().includes('@')

  return (
    <div className="mt-10">
      <h3 className="text-sm font-semibold text-text-primary mb-1">
        {t('blocked_title', { defaultValue: 'Adresses bloquées' })}
      </h3>
      <p className="text-xs text-text-tertiary mb-4">
        {t('blocked_hint', { defaultValue: 'Les messages des adresses bloquées sont automatiquement déplacés vers le spam.' })}
      </p>

      <div className="flex items-center gap-2 mb-4">
        <input
          type="email"
          value={email}
          onChange={e => setEmail(e.target.value)}
          onKeyDown={e => { if (e.key === 'Enter' && canAdd) addMut.mutate() }}
          placeholder={t('blocked_placeholder', { defaultValue: 'adresse@exemple.com' })}
          className="flex-1 border border-border rounded-lg px-3 py-2 text-sm text-text-primary
                     focus:outline-none focus:ring-2 focus:ring-primary/30"
        />
        <Button size="sm" onClick={() => addMut.mutate()} disabled={!canAdd} loading={addMut.isPending}>
          {t('blocked_add', { defaultValue: 'Bloquer' })}
        </Button>
      </div>

      {blockedQ.isLoading ? (
        <div className="py-6 flex justify-center"><Loader2 className="animate-spin text-text-tertiary" /></div>
      ) : blocked.length === 0 ? (
        <div className="py-8 text-center text-sm text-text-tertiary">
          {t('blocked_empty', { defaultValue: 'Aucune adresse bloquée.' })}
        </div>
      ) : (
        <div className="divide-y divide-border/50 border border-border rounded-lg">
          {blocked.map(b => (
            <div key={b.id} className="flex items-center gap-3 px-4 py-2.5">
              <Ban size={15} className="text-text-tertiary flex-shrink-0" />
              <span className="flex-1 text-sm text-text-primary truncate">{b.email}</span>
              <button onClick={() => delMut.mutate(b.id)} title={t('blocked_remove', { defaultValue: 'Débloquer' })}
                className="text-xs font-medium text-primary hover:underline flex-shrink-0">
                {t('blocked_remove', { defaultValue: 'Débloquer' })}
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
