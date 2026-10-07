import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { RefreshCw, Loader2, Check } from 'lucide-react'
import { mailApi } from '../api'
import { Button, Dropdown } from '@ui'
import { SettingsRow } from './SettingsRow'

// ── Anti-spam (Bayes) tab ─────────────────────────────────────────────────────

export function SpamTab() {
  const { t } = useTranslation('mail')
  const qc = useQueryClient()
  const { data, isLoading } = useQuery({ queryKey: ['mail-spam-stats'], queryFn: mailApi.getSpamStats })

  const settingsMut = useMutation({
    mutationFn: (dto: { auto_classify?: boolean; threshold?: number }) => mailApi.updateSpamSettings(dto),
    onSuccess:  () => qc.invalidateQueries({ queryKey: ['mail-spam-stats'] }),
  })

  const [trained, setTrained] = useState<{ spam_messages: number; ham_messages: number; capped: boolean } | null>(null)
  const trainMut = useMutation({
    mutationFn: () => mailApi.trainSpam(),
    onSuccess:  (res) => { setTrained(res); qc.invalidateQueries({ queryKey: ['mail-spam-stats'] }) },
  })

  if (isLoading || !data) {
    return <div className="py-16 flex justify-center"><Loader2 size={20} className="animate-spin text-text-tertiary" /></div>
  }

  // The threshold is exposed as understandable levels rather than a raw probability.
  const thresholdOptions = [
    { value: '0.99', label: t('spam_threshold_strict',   { defaultValue: 'Strict (peu de faux positifs)' }) },
    { value: '0.95', label: t('spam_threshold_balanced', { defaultValue: 'Équilibré (recommandé)' }) },
    { value: '0.85', label: t('spam_threshold_aggressive', { defaultValue: 'Agressif (attrape plus)' }) },
  ]
  const currentThreshold = thresholdOptions.find(o => Math.abs(Number(o.value) - data.threshold) < 0.001)?.value ?? '0.95'

  return (
    <div>
      <SettingsRow
        label={t('spam_auto_classify', { defaultValue: 'Filtrage automatique' })}
        description={t('spam_auto_classify_desc', { defaultValue: 'Déplacer automatiquement vers le dossier Spam les messages reconnus comme indésirables par le modèle bayésien personnel.' })}
      >
        <label className="flex items-center gap-2 cursor-pointer">
          <input
            type="checkbox"
            checked={data.auto_classify}
            onChange={e => settingsMut.mutate({ auto_classify: e.target.checked })}
          />
          <span className="text-sm text-text-primary">{t('spam_auto_classify_on', { defaultValue: 'Activer le tri automatique' })}</span>
        </label>
      </SettingsRow>

      <SettingsRow
        label={t('spam_threshold', { defaultValue: 'Sensibilité' })}
        description={t('spam_threshold_desc', { defaultValue: 'Niveau de confiance requis avant de déplacer un message vers le Spam.' })}
      >
        <div className="max-w-xs">
          <Dropdown
            value={currentThreshold}
            onChange={v => settingsMut.mutate({ threshold: Number(v) })}
            options={thresholdOptions}
          />
        </div>
      </SettingsRow>

      <SettingsRow
        label={t('spam_model', { defaultValue: 'Modèle d\'apprentissage' })}
        description={t('spam_model_desc', { defaultValue: 'Le modèle apprend de vos actions « Spam » / « Pas un spam ». Vous pouvez le reconstruire à partir de vos messages actuels.' })}
      >
        <div className="space-y-3">
          <div className="flex gap-4 text-sm text-text-secondary">
            <span><strong className="text-text-primary">{data.spam_messages}</strong> {t('spam_examples', { defaultValue: 'exemples spam' })}</span>
            <span><strong className="text-text-primary">{data.ham_messages}</strong> {t('ham_examples', { defaultValue: 'exemples légitimes' })}</span>
            <span><strong className="text-text-primary">{data.distinct_tokens}</strong> {t('spam_tokens', { defaultValue: 'mots appris' })}</span>
          </div>
          <Button onClick={() => trainMut.mutate()} disabled={trainMut.isPending} variant="secondary"
            icon={trainMut.isPending ? <Loader2 size={14} className="animate-spin" /> : <RefreshCw size={14} />}>
            {t('spam_retrain', { defaultValue: 'Réentraîner le modèle' })}
          </Button>
          {trained && (
            <p className="text-xs text-emerald-600 flex items-center gap-1">
              <Check size={14} />
              {t('spam_retrain_done', {
                defaultValue: 'Modèle reconstruit : {{spam}} spams, {{ham}} légitimes.',
                spam: trained.spam_messages, ham: trained.ham_messages,
              })}
            </p>
          )}
          {data.spam_messages + data.ham_messages < 20 && (
            <p className="text-xs text-amber-600">
              {t('spam_need_more', { defaultValue: 'Le tri automatique s\'active après ~20 exemples. Continuez à marquer vos spams.' })}
            </p>
          )}
        </div>
      </SettingsRow>
    </div>
  )
}
