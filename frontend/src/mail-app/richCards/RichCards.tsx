import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
  CalendarDays, Plane, BedDouble, TrainFront, Bus, UtensilsCrossed, Car,
  Package, MapPin, Clock, Check, CalendarPlus, Download, ExternalLink, Ticket,
} from 'lucide-react'
import { mailApi, type EmailMessage } from '../../api'
import { cardsFromNodes, type RichCard, type EventCard, type FlightCard, type LodgingCard, type TransitCard, type ReservationCard, type OrderCard } from './parse'
import { addEventToCalendar, downloadEventIcs, calendarService } from './actions'

type Rsvp = 'accepted' | 'tentative' | 'declined'
/** Context an event card needs to answer an invitation: which message it came
 *  from (for the iMIP reply endpoint) and the answer already given, if any. */
interface CardCtx { messageId?: string; inviteResponse?: Rsvp | null }

// ── date/time formatting (locale-aware, no extra dep) ────────────────────────
function useFmt() {
  const { i18n } = useTranslation()
  const loc = i18n.language || 'fr'
  return useMemo(() => ({
    dateTime: (iso?: string) => fmt(iso, loc, { weekday: 'short', day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' }),
    date:     (iso?: string) => fmt(iso, loc, { weekday: 'short', day: 'numeric', month: 'short', year: 'numeric' }),
    time:     (iso?: string) => fmt(iso, loc, { hour: '2-digit', minute: '2-digit' }),
  }), [loc])
}
function fmt(iso: string | undefined, loc: string, opts: Intl.DateTimeFormatOptions): string | undefined {
  if (!iso) return undefined
  const dateOnly = /^\d{4}-\d{2}-\d{2}$/.test(iso)
  const d = new Date(dateOnly ? `${iso}T00:00:00` : iso)
  if (Number.isNaN(d.getTime())) return iso
  const o = dateOnly ? { weekday: opts.weekday, day: 'numeric' as const, month: 'short' as const, year: opts.year } : opts
  try { return new Intl.DateTimeFormat(loc, o).format(d) } catch { return iso }
}

// ── shared chrome ────────────────────────────────────────────────────────────
function Shell({ icon, accent, children }: { icon: React.ReactNode; accent: string; children: React.ReactNode }) {
  return (
    <div className="flex gap-3 rounded-xl border border-border bg-surface-1/60 p-3 my-2">
      <div className="flex-shrink-0 w-9 h-9 rounded-lg grid place-items-center text-white" style={{ background: accent }}>
        {icon}
      </div>
      <div className="min-w-0 flex-1">{children}</div>
    </div>
  )
}
function Line({ icon, children }: { icon?: React.ReactNode; children: React.ReactNode }) {
  return (
    <div className="flex items-center gap-1.5 text-xs text-text-secondary mt-0.5 min-w-0">
      {icon}<span className="truncate">{children}</span>
    </div>
  )
}
function LinkBtn({ href, icon, children }: { href: string; icon: React.ReactNode; children: React.ReactNode }) {
  return (
    <a href={href} target="_blank" rel="noopener noreferrer"
      className="inline-flex items-center gap-1.5 h-8 px-3 rounded-md text-sm text-primary hover:bg-primary/10 transition-colors">
      {icon}{children}
    </a>
  )
}

// ── event card (invitation RSVP, or plain event + Add to calendar / .ics) ─────
function EventCardView({ c, ctx }: { c: EventCard; ctx?: CardCtx }) {
  const { t } = useTranslation('mail')
  const f = useFmt()
  const [added, setAdded] = useState(false)
  const [busy, setBusy] = useState<Rsvp | 'add' | null>(null)
  const [rsvp, setRsvp] = useState<Rsvp | null>(ctx?.inviteResponse ?? null)
  // Decline panel: an optional reason and an optional alternative time
  // (the latter turns the answer into an iTIP counter-proposal).
  const [declineOpen, setDeclineOpen] = useState(false)
  const [reason, setReason] = useState('')
  const [altStart, setAltStart] = useState('')
  const [altEnd, setAltEnd] = useState('')
  const hasCalendar = !!calendarService()
  const isInvite = !!c.isInvite && !!ctx?.messageId
  const when = c.end && c.start && sameDay(c.start, c.end)
    ? `${f.dateTime(c.start)} – ${f.time(c.end)}`
    : [f.dateTime(c.start), c.end && f.dateTime(c.end)].filter(Boolean).join(' → ')

  const add = async () => {
    setBusy('add')
    try { await addEventToCalendar(c); setAdded(true) } catch { /* calendar absent/failed */ } finally { setBusy(null) }
  }

  // RSVP to an invitation: email the organizer our answer (iMIP reply, backend)
  // and — for Yes/Maybe — mirror it into the calendar. Optimistic: the chosen
  // button lights up immediately; a failure reverts it. Declining opens a panel
  // first, where a reason and/or another time can be offered.
  const respond = async (answer: Rsvp, opts?: { comment?: string; proposedStart?: string; proposedEnd?: string }) => {
    if (!ctx?.messageId || busy) return
    const previous = rsvp
    setRsvp(answer)
    setBusy(answer)
    try {
      await mailApi.inviteReply(ctx.messageId, answer, opts)
      if (answer !== 'declined' && hasCalendar) {
        await addEventToCalendar(c, answer === 'accepted' ? 'confirmed' : 'tentative').catch(() => {})
      }
      setDeclineOpen(false)
    } catch {
      setRsvp(previous) // revert on failure
    } finally {
      setBusy(null)
    }
  }

  // Declining: the answer alone is enough, so the panel is entirely optional —
  // "Envoyer le refus" with both fields empty behaves like a plain No.
  const onPick = (answer: Rsvp) => {
    if (answer === 'declined') {
      setDeclineOpen(o => !o)
      return
    }
    setDeclineOpen(false)
    void respond(answer)
  }

  const RSVP_OPTIONS: { key: Rsvp; label: string }[] = [
    { key: 'accepted',  label: t('rc_yes',   { defaultValue: 'Oui' }) },
    { key: 'tentative', label: t('rc_maybe', { defaultValue: 'Peut-être' }) },
    { key: 'declined',  label: t('rc_no',    { defaultValue: 'Non' }) },
  ]

  return (
    <Shell icon={<CalendarDays size={18} />} accent="#1a73e8">
      <div className="flex items-center gap-2">
        <div className="font-medium text-sm text-text-primary truncate">{c.title}</div>
        {c.isInvite && (
          <span className="text-[11px] px-1.5 py-0.5 rounded bg-primary/10 text-primary flex-shrink-0">
            {t('rc_invite', { defaultValue: 'Invitation' })}
          </span>
        )}
      </div>
      {when && <Line icon={<Clock size={13} className="flex-shrink-0" />}>{when}</Line>}
      {(c.location || c.address) && <Line icon={<MapPin size={13} className="flex-shrink-0" />}>{[...new Set([c.location, c.address].filter(Boolean))].join(' · ')}</Line>}
      {c.organizer && <Line>{t('rc_organizer', { defaultValue: 'Organisé par' })} {c.organizer}</Line>}

      {isInvite ? (
        <div className="mt-2.5">
          <div className="text-xs text-text-secondary mb-1.5">{t('rc_going', { defaultValue: 'Participez-vous ?' })}</div>
          <div className="inline-flex rounded-lg border border-border overflow-hidden">
            {RSVP_OPTIONS.map((o, i) => {
              // Opening the decline panel IS choosing "No": light it up right
              // away rather than only once the reply leaves. While the panel is
              // open it is the only active choice, so a previous answer cannot
              // stay lit next to it.
              const selected = declineOpen ? o.key === 'declined' : rsvp === o.key
              return (
                <button
                  key={o.key}
                  type="button"
                  onClick={() => onPick(o.key)}
                  disabled={!!busy}
                  className={`h-8 px-4 text-sm transition-colors disabled:opacity-60 ${i > 0 ? 'border-l border-border' : ''} ${
                    selected ? 'bg-primary text-white' : 'text-text-secondary hover:bg-surface-2'
                  }`}
                >
                  {busy === o.key ? '…' : o.label}
                </button>
              )
            })}
          </div>

          {/* Declining: say why, and/or suggest another time (iTIP counter).
              Both are optional — sending with empty fields is a plain refusal. */}
          {declineOpen && (
            <div className="mt-2.5 p-3 rounded-lg border border-border bg-surface-1 max-w-lg space-y-2.5">
              <textarea
                value={reason}
                onChange={e => setReason(e.target.value)}
                rows={2}
                placeholder={t('rc_reason_ph', { defaultValue: 'Motif du refus (facultatif)' })}
                className="w-full rounded-md border border-border bg-surface-0 px-2.5 py-1.5 text-text-primary
                           placeholder:text-text-tertiary focus:outline-none focus:border-primary resize-y"
                style={{ fontSize: 14 }}
              />
              <div>
                <div className="text-xs text-text-secondary mb-1">
                  {t('rc_propose', { defaultValue: 'Proposer un autre horaire (facultatif)' })}
                </div>
                <div className="flex items-center gap-2 flex-wrap">
                  <input
                    type="datetime-local"
                    value={altStart}
                    onChange={e => setAltStart(e.target.value)}
                    className="rounded-md border border-border bg-surface-0 px-2 py-1.5 text-text-primary focus:outline-none focus:border-primary"
                    style={{ fontSize: 14 }}
                  />
                  <span className="text-text-tertiary text-sm">→</span>
                  <input
                    type="datetime-local"
                    value={altEnd}
                    onChange={e => setAltEnd(e.target.value)}
                    className="rounded-md border border-border bg-surface-0 px-2 py-1.5 text-text-primary focus:outline-none focus:border-primary"
                    style={{ fontSize: 14 }}
                  />
                </div>
              </div>
              <div className="flex items-center gap-2">
                <button
                  type="button"
                  disabled={!!busy}
                  onClick={() => respond('declined', {
                    comment:       reason.trim() || undefined,
                    proposedStart: altStart || undefined,
                    proposedEnd:   altEnd || undefined,
                  })}
                  className="h-8 px-3 rounded-md text-sm bg-primary text-white hover:bg-primary-hover disabled:opacity-60 transition-colors"
                >
                  {altStart
                    ? t('rc_send_counter', { defaultValue: 'Proposer ce créneau' })
                    : t('rc_send_decline', { defaultValue: 'Envoyer le refus' })}
                </button>
                <button
                  type="button"
                  onClick={() => setDeclineOpen(false)}
                  className="h-8 px-3 rounded-md text-sm text-text-secondary hover:bg-surface-2 transition-colors"
                >
                  {t('common_cancel', { defaultValue: 'Annuler' })}
                </button>
              </div>
            </div>
          )}

          {rsvp && !declineOpen && (
            <div className="flex items-center gap-1.5 text-xs text-text-tertiary mt-1.5">
              <Check size={13} className="text-success" />
              {rsvp === 'accepted'  && t('rc_replied_yes',   { defaultValue: 'Vous avez accepté — l\'organisateur a été prévenu.' })}
              {rsvp === 'tentative' && t('rc_replied_maybe', { defaultValue: 'Réponse « peut-être » envoyée à l\'organisateur.' })}
              {rsvp === 'declined'  && t('rc_replied_no',    { defaultValue: 'Vous avez refusé — l\'organisateur a été prévenu.' })}
            </div>
          )}
        </div>
      ) : (
        <div className="flex items-center gap-1 mt-2 -ml-1 flex-wrap">
          {hasCalendar && (
            <button type="button" onClick={add} disabled={busy === 'add' || added}
              className="inline-flex items-center gap-1.5 h-8 px-3 rounded-md text-sm text-primary hover:bg-primary/10 disabled:opacity-60 transition-colors">
              {added ? <><Check size={15} />{t('rc_added', { defaultValue: 'Ajouté à l\'agenda' })}</>
                     : <><CalendarPlus size={15} />{t('rc_add', { defaultValue: 'Ajouter à l\'agenda' })}</>}
            </button>
          )}
          <button type="button" onClick={() => downloadEventIcs(c)}
            className="inline-flex items-center gap-1.5 h-8 px-3 rounded-md text-sm text-text-secondary hover:bg-surface-2 transition-colors">
            <Download size={15} />{t('rc_ics', { defaultValue: '.ics' })}
          </button>
          {c.url && <LinkBtn href={c.url} icon={<ExternalLink size={15} />}>{t('rc_details', { defaultValue: 'Détails' })}</LinkBtn>}
        </div>
      )}
    </Shell>
  )
}

// ── flight card ──────────────────────────────────────────────────────────────
function FlightCardView({ c }: { c: FlightCard }) {
  const { t } = useTranslation('mail')
  const f = useFmt()
  const num = [c.iata, c.flightNumber].filter(Boolean).join(' ')
  const airport = (a: FlightCard['from']) => a.code || a.name || '—'
  return (
    <Shell icon={<Plane size={18} />} accent="#5b3fd6">
      <div className="flex items-center gap-2 flex-wrap">
        <div className="font-medium text-sm text-text-primary">{c.airline || t('rc_flight', { defaultValue: 'Vol' })}</div>
        {num && <span className="text-xs text-text-secondary">{num}</span>}
        {c.reservationId && <span className="text-[11px] px-1.5 py-0.5 rounded bg-surface-2 text-text-secondary">{c.reservationId}</span>}
      </div>
      <div className="flex items-center gap-2 mt-1 text-sm text-text-primary">
        <span className="font-medium">{airport(c.from)}</span>
        <span className="text-text-tertiary">→</span>
        <span className="font-medium">{airport(c.to)}</span>
      </div>
      <div className="grid grid-cols-2 gap-x-4 mt-1">
        <Line icon={<Clock size={13} className="flex-shrink-0" />}>
          {f.dateTime(c.from.time)}{c.from.terminal ? ` · T${c.from.terminal}` : ''}{c.from.gate ? ` · ${t('rc_gate', { defaultValue: 'Porte' })} ${c.from.gate}` : ''}
        </Line>
        <Line icon={<Clock size={13} className="flex-shrink-0" />}>
          {f.dateTime(c.to.time)}{c.to.terminal ? ` · T${c.to.terminal}` : ''}
        </Line>
      </div>
      {(c.seat || c.boardingGroup) && (
        <Line icon={<Ticket size={13} className="flex-shrink-0" />}>
          {[c.seat && `${t('rc_seat', { defaultValue: 'Siège' })} ${c.seat}`, c.boardingGroup && `${t('rc_group', { defaultValue: 'Groupe' })} ${c.boardingGroup}`].filter(Boolean).join(' · ')}
        </Line>
      )}
      {c.checkinUrl && (
        <div className="-ml-1 mt-2">
          <LinkBtn href={c.checkinUrl} icon={<ExternalLink size={15} />}>{t('rc_checkin', { defaultValue: 'Enregistrement en ligne' })}</LinkBtn>
        </div>
      )}
    </Shell>
  )
}

// ── lodging card ─────────────────────────────────────────────────────────────
function LodgingCardView({ c }: { c: LodgingCard }) {
  const { t } = useTranslation('mail')
  const f = useFmt()
  return (
    <Shell icon={<BedDouble size={18} />} accent="#0a7c66">
      <div className="font-medium text-sm text-text-primary truncate">{c.name || t('rc_hotel', { defaultValue: 'Hébergement' })}</div>
      {c.address && <Line icon={<MapPin size={13} className="flex-shrink-0" />}>{c.address}</Line>}
      {(c.checkin || c.checkout) && (
        <Line icon={<Clock size={13} className="flex-shrink-0" />}>
          {[c.checkin && `${t('rc_checkin_date', { defaultValue: 'Arrivée' })} ${f.date(c.checkin)}`, c.checkout && `${t('rc_checkout_date', { defaultValue: 'Départ' })} ${f.date(c.checkout)}`].filter(Boolean).join(' · ')}
        </Line>
      )}
      {c.url && <div className="-ml-1 mt-2"><LinkBtn href={c.url} icon={<ExternalLink size={15} />}>{t('rc_details', { defaultValue: 'Détails' })}</LinkBtn></div>}
    </Shell>
  )
}

// ── transit (train/bus) card ─────────────────────────────────────────────────
function TransitCardView({ c }: { c: TransitCard }) {
  const { t } = useTranslation('mail')
  const f = useFmt()
  return (
    <Shell icon={c.mode === 'train' ? <TrainFront size={18} /> : <Bus size={18} />} accent="#b45309">
      <div className="flex items-center gap-2 flex-wrap">
        <div className="font-medium text-sm text-text-primary">{c.operator || (c.mode === 'train' ? t('rc_train', { defaultValue: 'Train' }) : t('rc_bus', { defaultValue: 'Bus' }))}</div>
        {c.number && <span className="text-xs text-text-secondary">{c.number}</span>}
      </div>
      <div className="flex items-center gap-2 mt-1 text-sm text-text-primary">
        <span className="font-medium truncate">{c.from.name || '—'}</span>
        <span className="text-text-tertiary">→</span>
        <span className="font-medium truncate">{c.to.name || '—'}</span>
      </div>
      <div className="grid grid-cols-2 gap-x-4">
        <Line icon={<Clock size={13} className="flex-shrink-0" />}>{f.dateTime(c.from.time)}</Line>
        <Line icon={<Clock size={13} className="flex-shrink-0" />}>{f.dateTime(c.to.time)}</Line>
      </div>
    </Shell>
  )
}

// ── restaurant / car reservation card ────────────────────────────────────────
function ReservationCardView({ c }: { c: ReservationCard }) {
  const { t } = useTranslation('mail')
  const f = useFmt()
  const isResto = c.variant === 'restaurant'
  return (
    <Shell icon={isResto ? <UtensilsCrossed size={18} /> : <Car size={18} />} accent={isResto ? '#c026a3' : '#4b5563'}>
      <div className="font-medium text-sm text-text-primary truncate">{c.name || (isResto ? t('rc_resto', { defaultValue: 'Restaurant' }) : t('rc_car', { defaultValue: 'Location de voiture' }))}</div>
      {c.time && <Line icon={<Clock size={13} className="flex-shrink-0" />}>{f.dateTime(c.time)}{c.partySize ? ` · ${c.partySize} ${t('rc_guests', { defaultValue: 'pers.' })}` : ''}</Line>}
      {c.address && <Line icon={<MapPin size={13} className="flex-shrink-0" />}>{c.address}</Line>}
      {c.url && <div className="-ml-1 mt-2"><LinkBtn href={c.url} icon={<ExternalLink size={15} />}>{t('rc_details', { defaultValue: 'Détails' })}</LinkBtn></div>}
    </Shell>
  )
}

// ── order / parcel card ──────────────────────────────────────────────────────
function OrderCardView({ c }: { c: OrderCard }) {
  const { t } = useTranslation('mail')
  return (
    <Shell icon={<Package size={18} />} accent="#1f6feb">
      <div className="flex items-center gap-2 flex-wrap">
        <div className="font-medium text-sm text-text-primary truncate">{c.seller || t('rc_order', { defaultValue: 'Commande' })}</div>
        {c.orderNumber && <span className="text-xs text-text-secondary">#{c.orderNumber}</span>}
        {c.total && <span className="text-xs text-text-secondary">{c.total}</span>}
      </div>
      {c.deliveryTo && <Line icon={<MapPin size={13} className="flex-shrink-0" />}>{c.deliveryTo}</Line>}
      {c.trackingUrl && (
        <div className="-ml-1 mt-2">
          <LinkBtn href={c.trackingUrl} icon={<ExternalLink size={15} />}>{t('rc_track', { defaultValue: 'Suivre le colis' })}</LinkBtn>
        </div>
      )}
    </Shell>
  )
}

function CardView({ c, ctx }: { c: RichCard; ctx?: CardCtx }) {
  switch (c.kind) {
    case 'event':       return <EventCardView c={c} ctx={ctx} />
    case 'flight':      return <FlightCardView c={c} />
    case 'lodging':     return <LodgingCardView c={c} />
    case 'transit':     return <TransitCardView c={c} />
    case 'reservation': return <ReservationCardView c={c} />
    case 'order':       return <OrderCardView c={c} />
  }
}

/** Gmail-style rich cards rendered above the email body: parses the message's
 *  schema.org nodes (calendar events/invites, flights, hotels, transit,
 *  restaurant/car, orders) and renders one card each. Renders nothing when the
 *  message carries no recognized structured data. */
export default function RichCards({ message }: { message: EmailMessage }) {
  const cards = useMemo(() => cardsFromNodes(message.structured_data), [message.structured_data])
  if (!cards.length) return null
  const ctx: CardCtx = { messageId: message.id, inviteResponse: message.invite_response ?? null }
  return (
    <div className="mb-2">
      {cards.map((c, i) => <CardView key={i} c={c} ctx={ctx} />)}
    </div>
  )
}

function sameDay(a: string, b: string): boolean {
  return new Date(a).toDateString() === new Date(b).toDateString()
}
