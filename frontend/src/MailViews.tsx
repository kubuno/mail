// Vues dédiées : « Planifié » (brouillons programmés) et « Gérer les abonnements ».
import { useTranslation } from 'react-i18next'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { CalendarClock, MailX, ExternalLink, Loader2 } from 'lucide-react'
import { mailApi } from './api'

function fmtDate(s: string, lang: string) {
  return new Date(s).toLocaleString(lang, { dateStyle: 'medium', timeStyle: 'short' })
}

// ── Planifié ────────────────────────────────────────────────────────────────
export function ScheduledView() {
  const { t, i18n } = useTranslation('mail')
  const { data: scheduled = [], isLoading } = useQuery({
    queryKey: ['mail-scheduled'], queryFn: mailApi.getScheduled, refetchInterval: 60_000,
  })

  return (
    <div className="flex-1 flex flex-col overflow-hidden">
      <div className="flex items-center gap-2 px-6 py-4 border-b border-[#e0e0e0] flex-shrink-0">
        <CalendarClock size={18} className="text-text-secondary" />
        <h2 className="text-base font-medium text-text-primary">{t('folder_scheduled', { defaultValue: 'Planifié' })}</h2>
      </div>
      {isLoading ? (
        <div className="flex-1 flex items-center justify-center"><Loader2 className="animate-spin text-text-tertiary" /></div>
      ) : scheduled.length === 0 ? (
        <div className="flex-1 flex items-center justify-center text-text-tertiary text-sm">
          {t('scheduled_empty', { defaultValue: 'Aucun message programmé.' })}
        </div>
      ) : (
        <div className="flex-1 overflow-y-auto divide-y divide-border/40">
          {scheduled.map(s => (
            <div key={s.id} className="flex items-center gap-4 px-6 py-3 hover:bg-surface-1">
              <div className="flex-1 min-w-0">
                <div className="text-sm font-medium text-text-primary truncate">{s.subject || t('no_subject', { defaultValue: '(sans objet)' })}</div>
                <div className="text-xs text-text-tertiary truncate">
                  {(s.to_addresses ?? []).map(a => a.email).join(', ')}
                </div>
              </div>
              <div className="flex items-center gap-1.5 text-xs text-primary whitespace-nowrap flex-shrink-0">
                <CalendarClock size={13} />
                {fmtDate(s.scheduled_at, i18n.language)}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}

// ── Gérer les abonnements ─────────────────────────────────────────────────────
// Exporté : réutilisé par le bouton « Se désabonner » du lecteur de message.
export function unsubscribeTarget(raw: string): string | null {
  // List-Unsubscribe : "<https://...>, <mailto:...>" → préférer l'URL http.
  const links = [...raw.matchAll(/<([^>]+)>/g)].map(m => m[1])
  return links.find(l => /^https?:/i.test(l)) ?? links.find(l => /^mailto:/i.test(l)) ?? null
}

export function SubscriptionsView() {
  const { t, i18n } = useTranslation('mail')
  const qc = useQueryClient()
  const { data: subs = [], isLoading } = useQuery({
    queryKey: ['mail-subscriptions'], queryFn: mailApi.getSubscriptions,
  })

  return (
    <div className="flex-1 flex flex-col overflow-hidden">
      <div className="flex items-center gap-2 px-6 py-4 border-b border-[#e0e0e0] flex-shrink-0">
        <MailX size={18} className="text-text-secondary" />
        <h2 className="text-base font-medium text-text-primary">{t('folder_subscriptions', { defaultValue: 'Gérer les abonnements' })}</h2>
      </div>
      {isLoading ? (
        <div className="flex-1 flex items-center justify-center"><Loader2 className="animate-spin text-text-tertiary" /></div>
      ) : subs.length === 0 ? (
        <div className="flex-1 flex items-center justify-center text-text-tertiary text-sm">
          {t('subs_empty', { defaultValue: 'Aucun abonnement détecté (en-tête List-Unsubscribe).' })}
        </div>
      ) : (
        <div className="flex-1 overflow-y-auto divide-y divide-border/40">
          {subs.map(s => {
            const target = s.list_unsubscribe ? unsubscribeTarget(s.list_unsubscribe) : null
            return (
              <div key={s.from_email} className="flex items-center gap-4 px-6 py-3 hover:bg-surface-1">
                <div className="flex-1 min-w-0">
                  <div className="text-sm font-medium text-text-primary truncate">{s.from_name || s.from_email}</div>
                  <div className="text-xs text-text-tertiary truncate">{s.from_email}</div>
                </div>
                <div className="text-xs text-text-tertiary whitespace-nowrap flex-shrink-0">
                  {t('subs_count', { count: s.count, defaultValue: `${s.count} messages` })}
                  <span className="mx-2">·</span>
                  {fmtDate(s.last_at, i18n.language)}
                </div>
                <button
                  disabled={!target}
                  onClick={() => {
                    if (!target) return
                    if (target.startsWith('mailto:')) window.location.href = target
                    else window.open(target, '_blank', 'noopener,noreferrer')
                    // L'abonnement disparaîtra au prochain sync s'il n'envoie plus.
                    qc.invalidateQueries({ queryKey: ['mail-subscriptions'] })
                  }}
                  className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-sm border border-border text-text-secondary
                             hover:bg-danger/10 hover:text-danger hover:border-danger/30 disabled:opacity-40 transition-colors flex-shrink-0"
                  title={target ?? ''}
                >
                  <ExternalLink size={14} />
                  {t('subs_unsubscribe', { defaultValue: 'Se désabonner' })}
                </button>
              </div>
            )
          })}
        </div>
      )}
    </div>
  )
}
