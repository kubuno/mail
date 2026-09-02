import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
  CalendarDays, Plane, BedDouble, TrainFront, Bus, UtensilsCrossed, Car,
  Package, MapPin, Clock, Check, CalendarPlus, Download, ExternalLink, Ticket,
} from 'lucide-react'
import type { EmailMessage } from '../../api'
import { cardsFromNodes, type RichCard, type EventCard, type FlightCard, type LodgingCard, type TransitCard, type ReservationCard, type OrderCard } from './parse'
import { addEventToCalendar, downloadEventIcs, calendarService } from './actions'

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

// ── event card (+ Add to calendar / .ics) ────────────────────────────────────
function EventCardView({ c }: { c: EventCard }) {
  const { t } = useTranslation('mail')
  const f = useFmt()
  const [added, setAdded] = useState(false)
  const [busy, setBusy] = useState(false)
  const hasCalendar = !!calendarService()
  const when = c.end && c.start && sameDay(c.start, c.end)
    ? `${f.dateTime(c.start)} – ${f.time(c.end)}`
    : [f.dateTime(c.start), c.end && f.dateTime(c.end)].filter(Boolean).join(' → ')

  const add = async () => {
    setBusy(true)
    try { await addEventToCalendar(c); setAdded(true) } catch { /* calendar absent/failed */ } finally { setBusy(false) }
  }
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
      <div className="flex items-center gap-1 mt-2 -ml-1 flex-wrap">
        {hasCalendar && (
          <button type="button" onClick={add} disabled={busy || added}
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

function CardView({ c }: { c: RichCard }) {
  switch (c.kind) {
    case 'event':       return <EventCardView c={c} />
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
  return (
    <div className="mb-2">
      {cards.map((c, i) => <CardView key={i} c={c} />)}
    </div>
  )
}

function sameDay(a: string, b: string): boolean {
  return new Date(a).toDateString() === new Date(b).toDateString()
}
