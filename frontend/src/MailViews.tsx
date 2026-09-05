// Vues dédiées : « Brouillons » (mail.drafts), « Planifié » (brouillons programmés)
// et « Gérer les abonnements ».
import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { CalendarClock, MailX, ExternalLink, Loader2, RefreshCw, Paperclip, Trash2 } from 'lucide-react'
import { useNavigate } from 'react-router-dom'
import { useIsMobile, ConfirmDialog } from '@ui'
import { useConfirm } from '@kubuno/sdk'
import { mailApi, Draft, type Subscription } from './api'
import SenderAvatar from './mail-app/SenderAvatar'
import { analyzeSender } from './mail-app/senderSafety'
import { useMailStore } from './store'
import { SelectBox, DragGrip } from './mail-app/rowChrome'
import MailFolderFilterBar, { type FilterState, EMPTY_FILTER } from './mail-app/MailFolderFilterBar'

function fmtDate(s: string, lang: string) {
  return new Date(s).toLocaleString(lang, { dateStyle: 'medium', timeStyle: 'short' })
}

/** Short day/time à la Gmail for a list row. */
function fmtRowDate(s: string, lang: string) {
  const d = new Date(s)
  const now = new Date()
  const sameYear = d.getFullYear() === now.getFullYear()
  const sameDay = d.toDateString() === now.toDateString()
  if (sameDay) return d.toLocaleTimeString(lang, { hour: '2-digit', minute: '2-digit' })
  return d.toLocaleDateString(lang, sameYear ? { day: 'numeric', month: 'short' } : { day: 'numeric', month: 'short', year: 'numeric' })
}

/** Plain-text preview of a draft body (the stored value is HTML). */
function bodyPreview(html: string): string {
  const text = html.replace(/<[^>]*>/g, ' ').replace(/&nbsp;/gi, ' ').replace(/\s+/g, ' ').trim()
  return text.length > 200 ? text.slice(0, 200) + '…' : text
}

const DAYS: Record<string, number> = { '7d': 7, '1m': 30, '6m': 180, '1y': 365 }

/** Client-side filtering of the local drafts (there is no draft search index). */
function draftMatches(d: Draft, f: FilterState): boolean {
  if (f.to.trim()) {
    const q = f.to.trim().toLowerCase()
    if (!d.to_addresses.some(a => `${a.email} ${a.name ?? ''}`.toLowerCase().includes(q))) return false
  }
  if (f.attach && d.attachments.length === 0) return false
  if (f.noAgenda && d.attachments.some(a => /\.ics$/i.test(a.name))) return false
  const upd = new Date(d.updated_at).getTime()
  if (f.date === 'custom') {
    if (f.after && upd < new Date(f.after).getTime()) return false
    if (f.before && upd > new Date(f.before).getTime() + 86_400_000) return false
  } else if (f.date && DAYS[f.date]) {
    // « Plus d'une semaine » = modifié il y a plus de N jours.
    if (upd > Date.now() - DAYS[f.date] * 86_400_000) return false
  }
  return true
}

// ── Brouillons ────────────────────────────────────────────────────────────────
// The local drafts live in `mail.drafts` (auto-saved compose sessions), NOT in
// the synced message store — so this folder has its own list and cannot reuse
// the thread list. Clicking a row resumes editing the SAME draft; the only ways
// to remove one are this explicit trash button and sending it.
export function DraftsView() {
  const { t, i18n } = useTranslation('mail')
  const qc = useQueryClient()
  const isMobile = useIsMobile()
  const { setComposeInitial, setComposeOpen } = useMailStore()
  const { data, isLoading, isFetching } = useQuery({
    queryKey: ['mail-drafts'], queryFn: mailApi.listDrafts,
  })
  const allDrafts = data?.drafts ?? []

  const [filter, setFilter] = useState<FilterState>(EMPTY_FILTER)
  const [checked, setChecked] = useState<Set<string>>(new Set())

  const drafts = useMemo(() => allDrafts.filter(d => draftMatches(d, filter)), [allDrafts, filter])
  const allChecked = drafts.length > 0 && drafts.every(d => checked.has(d.id))

  const resume = (d: Draft) => {
    setComposeInitial({
      to: d.to_addresses, cc: d.cc_addresses, bcc: d.bcc_addresses,
      subject: d.subject, bodyHtml: d.body_html,
      draftId: d.id, replyToId: d.reply_to_id ?? undefined,
    })
    setComposeOpen(true)
  }

  const invalidate = () => {
    qc.invalidateQueries({ queryKey: ['mail-drafts'] })
    qc.invalidateQueries({ queryKey: ['mail-counts'] })
  }
  const remove = (ids: string[]) => {
    Promise.all(ids.map(id => mailApi.deleteDraft(id).catch(() => undefined))).then(invalidate)
    setChecked(prev => { const n = new Set(prev); ids.forEach(i => n.delete(i)); return n })
  }
  const toggleOne = (id: string) => setChecked(prev => {
    const n = new Set(prev); n.has(id) ? n.delete(id) : n.add(id); return n
  })
  const toggleAll = () => setChecked(allChecked ? new Set() : new Set(drafts.map(d => d.id)))

  return (
    <div className="flex flex-col bg-white overflow-hidden flex-1 min-w-0">
      {/* Toolbar — mirrors the inbox: select-all, refresh (or bulk delete). */}
      <div className="flex items-center gap-1 px-3 h-12 border-b border-[#e0e0e0] flex-shrink-0">
        <div className="w-10 flex items-center justify-center">
          <SelectBox checked={allChecked} partial={checked.size > 0 && !allChecked} onClick={toggleAll}
            label={t('mail_select_all', { defaultValue: 'Tout sélectionner' })} />
        </div>
        {checked.size > 0 ? (
          <button
            onClick={() => remove([...checked])}
            className="inline-flex items-center gap-1.5 h-8 px-3 rounded-md text-sm text-danger hover:bg-danger/10"
          >
            <Trash2 size={16} /> {t('delete', { defaultValue: 'Supprimer' })} ({checked.size})
          </button>
        ) : (
          <button onClick={() => qc.invalidateQueries({ queryKey: ['mail-drafts'] })}
            className="p-2 rounded-full text-text-secondary hover:bg-surface-1" title={t('mail_refresh', { defaultValue: 'Actualiser' })}>
            <RefreshCw size={16} className={isFetching ? 'animate-spin' : ''} />
          </button>
        )}
        <div className="flex-1" />
        {drafts.length > 0 && (
          <span className="text-xs text-text-tertiary pr-2">{t('drafts_count', { count: drafts.length, defaultValue: `${drafts.length} brouillon(s)` })}</span>
        )}
      </div>

      <MailFolderFilterBar folder="drafts" onChange={setFilter} />

      {isLoading ? (
        <div className="flex-1 flex items-center justify-center"><Loader2 className="animate-spin text-text-tertiary" /></div>
      ) : drafts.length === 0 ? (
        <div className="flex-1 flex items-center justify-center text-text-tertiary text-sm">
          {t('drafts_empty', { defaultValue: 'Aucun brouillon.' })}
        </div>
      ) : (
        <div className="flex-1 overflow-y-auto">
          {drafts.map(d => {
            const recipients = d.to_addresses.map(a => a.name || a.email).join(', ')
            const preview = bodyPreview(d.body_html)
            const isChecked = checked.has(d.id)
            return (
              <div
                key={d.id}
                role="button"
                tabIndex={0}
                onClick={() => resume(d)}
                onKeyDown={e => { if (e.key === 'Enter') resume(d) }}
                className={`group relative flex items-center gap-3 pl-4 pr-4 min-h-[40px] py-1.5 cursor-pointer
                            border-b border-[#f0f0f0] ${isChecked ? 'bg-[#c2dbff]' : 'hover:shadow-[inset_0_-1px_0_0_rgba(100,121,143,0.12)] hover:z-10 hover:bg-white'}`}
              >
                {/* Drag grip — appears on hover in the left gutter slot, mirroring
                    the inbox rows. Purely decorative here: drafts don't reorder
                    like conversation threads (which drag onto sidebar labels), so
                    no drag handler is wired — only the hover affordance. */}
                {!isMobile && (
                  <span
                    onClick={e => e.stopPropagation()}
                    aria-hidden="true"
                    className="absolute left-[3px] w-[10px] h-5 flex items-center justify-center
                               cursor-grab active:cursor-grabbing opacity-0 group-hover:opacity-100"
                  >
                    <DragGrip />
                  </span>
                )}

                {/* Gutter: selection box (star omitted — drafts aren't starrable). */}
                <div className="w-5 flex items-center justify-center flex-shrink-0" onClick={e => e.stopPropagation()}>
                  <SelectBox checked={isChecked} onClick={() => toggleOne(d.id)}
                    label={t('mail_select_thread', { defaultValue: 'Sélectionner' })} />
                </div>

                {/* Red « Brouillon » label + recipients (like Gmail). */}
                <div className={`flex items-baseline gap-2 flex-shrink-0 ${isMobile ? 'w-auto' : 'w-52'}`}>
                  <span className="text-sm text-danger">{t('mail_draft_label', { defaultValue: 'Brouillon' })}</span>
                  {recipients && !isMobile && <span className="text-sm text-text-secondary truncate">{recipients}</span>}
                </div>

                {/* Subject — snippet. */}
                <div className="flex-1 min-w-0 text-sm truncate">
                  <span className="text-text-primary">{d.subject || t('no_subject', { defaultValue: '(sans objet)' })}</span>
                  {preview && <span className="text-text-tertiary"> — {preview}</span>}
                </div>

                {d.attachments.length > 0 && <Paperclip size={14} className="text-text-tertiary flex-shrink-0" />}

                {/* Date, replaced by a trash button on hover (desktop). */}
                <div className="w-24 flex items-center justify-end flex-shrink-0">
                  <span className="text-xs text-text-tertiary group-hover:hidden">{fmtRowDate(d.updated_at, i18n.language)}</span>
                  <button
                    onClick={e => { e.stopPropagation(); remove([d.id]) }}
                    className="hidden group-hover:inline-flex p-1.5 rounded-full hover:bg-danger/10 hover:text-danger text-text-tertiary"
                    title={t('discard', { defaultValue: 'Supprimer le brouillon' })}
                  >
                    <Trash2 size={16} />
                  </button>
                </div>
              </div>
            )
          })}
        </div>
      )}
    </div>
  )
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
  const { t } = useTranslation('mail')
  const qc = useQueryClient()
  const navigate = useNavigate()
  const { setSearchQuery } = useMailStore()
  const { confirm, confirmState, handleConfirm, handleCancel } = useConfirm()
  const { data: subs = [], isLoading } = useQuery({
    queryKey: ['mail-subscriptions'], queryFn: mailApi.getSubscriptions,
  })

  // Clicking a subscription row opens the mailbox on the FULL Gmail-equivalent
  // query — Gmail issues `from:X (-label:spam OR label:trash) label:^sub_m`;
  // ours transposes it operator for operator: everywhere but spam (trash
  // included), subscription-type messages only (`is:subscription` = carries a
  // List-Unsubscribe header, our equivalent of the `^sub_m` system label).
  // Navigating to /mail mounts ThreadList without clearing the search.
  const openSenderSearch = (email: string) => {
    setSearchQuery(`from:${email} (-in:spam OR in:trash) is:subscription`)
    navigate('/mail')
  }

  // Gmail-style frequency bucket derived from the sender's message count.
  const freqLabel = (count: number) =>
    count > 20 ? t('subs_freq_many', { defaultValue: 'Plus de 20 e-mails récemment' })
    : count >= 10 ? t('subs_freq_med', { defaultValue: '10-20 e-mails récemment' })
    : t('subs_freq_few', { count, defaultValue: `${count} e-mail${count > 1 ? 's' : ''} récemment` })

  // Ask before unsubscribing (Gmail flow), then trigger the List-Unsubscribe.
  const doUnsubscribe = async (s: Subscription) => {
    // The confirmation quotes the sender: never with a raw display name, which
    // could carry bidi marks or claim someone else's address.
    const name = analyzeSender(s.from_name, s.from_email).label
    const ok = await confirm({
      title:        t('subs_unsubscribe', { defaultValue: 'Se désabonner' }),
      message:      t('subs_confirm', { name, email: s.from_email,
                      defaultValue: `Voulez-vous arrêter de recevoir des messages de toutes les listes de diffusion de ${name} (${s.from_email}) ?` }),
      confirmLabel: t('subs_unsubscribe', { defaultValue: 'Se désabonner' }),
    })
    if (!ok) return
    const target = s.list_unsubscribe ? unsubscribeTarget(s.list_unsubscribe) : null
    if (target) {
      if (target.startsWith('mailto:')) window.location.href = target
      else window.open(target, '_blank', 'noopener,noreferrer')
    }
    // The subscription drops off at the next sync once it stops sending.
    qc.invalidateQueries({ queryKey: ['mail-subscriptions'] })
  }

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
        <div className="flex-1 overflow-y-auto">
          <p className="px-6 py-4 text-sm text-text-tertiary">
            {t('subs_intro', { defaultValue: 'Lorsque vous vous désabonnez, vous pouvez continuer à recevoir des messages pendant quelques jours' })}
          </p>
          <div className="divide-y divide-border/40">
            {subs.map(s => (
              <div key={s.from_email} className="group flex items-center gap-4 px-6 py-2.5 hover:bg-surface-1">
                {/* The sender area is clickable: it opens the mailbox filtered on
                    this sender (all their messages). The unsubscribe stays apart. */}
                <button
                  type="button"
                  onClick={() => openSenderSearch(s.from_email)}
                  title={t('subs_view_messages', { defaultValue: `Voir les messages de ${analyzeSender(s.from_name, s.from_email).label}` })}
                  className="flex-1 flex items-center gap-4 min-w-0 text-left"
                >
                  <SenderAvatar email={s.from_email} name={s.from_name} size={28} />
                  {/* The address always sits next to it in this list, so the
                      name only needs neutralizing — `label` still swaps in the
                      address when the name impersonates another one. */}
                  <span className="w-56 min-w-0 flex-shrink-0 text-sm text-text-primary truncate">
                    {analyzeSender(s.from_name, s.from_email).label}
                  </span>
                  <span className="flex-1 min-w-0 text-sm text-text-secondary truncate">{s.from_email}</span>
                  <span className="text-sm text-text-secondary whitespace-nowrap flex-shrink-0">{freqLabel(s.count)}</span>
                </button>
                <button
                  onClick={() => doUnsubscribe(s)}
                  title={t('subs_unsubscribe', { defaultValue: 'Se désabonner' })}
                  className="text-sm text-text-primary px-3 py-1.5 rounded-full hover:bg-surface-2 transition-colors flex-shrink-0"
                >
                  {t('subs_unsubscribe', { defaultValue: 'Se désabonner' })}
                </button>
              </div>
            ))}
          </div>
        </div>
      )}
      {confirmState && <ConfirmDialog {...confirmState} onConfirm={handleConfirm} onCancel={handleCancel} />}
    </div>
  )
}
