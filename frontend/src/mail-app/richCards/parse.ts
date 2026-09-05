// Normalize the schema.org nodes the backend extracts (JSON-LD annotations + ICS
// invites) into a small set of view models, one per Gmail-style card. The nodes
// arrive loosely typed (Record<string, unknown>); we read defensively.
import type { SchemaNode } from '../../api'

export type RichCard =
  | EventCard
  | FlightCard
  | LodgingCard
  | TransitCard
  | ReservationCard
  | OrderCard

interface Base { reservationId?: string; underName?: string; fromIcs?: boolean }

export interface EventCard extends Base {
  kind: 'event'
  title: string
  start?: string
  end?: string
  location?: string
  address?: string
  description?: string
  url?: string
  isInvite?: boolean
  organizer?: string
  organizerEmail?: string
  uid?: string
}
export interface FlightCard extends Base {
  kind: 'flight'
  airline?: string
  iata?: string
  flightNumber?: string
  from: Airport
  to: Airport
  seat?: string
  boardingGroup?: string
  checkinUrl?: string
}
interface Airport { code?: string; name?: string; time?: string; terminal?: string; gate?: string }
export interface LodgingCard extends Base {
  kind: 'lodging'
  name?: string
  address?: string
  checkin?: string
  checkout?: string
  url?: string
}
export interface TransitCard extends Base {
  kind: 'transit'
  mode: 'train' | 'bus'
  operator?: string
  number?: string
  from: { name?: string; time?: string }
  to: { name?: string; time?: string }
}
export interface ReservationCard extends Base {
  kind: 'reservation'
  variant: 'restaurant' | 'car'
  name?: string
  time?: string
  address?: string
  partySize?: number
  url?: string
}
export interface OrderCard extends Base {
  kind: 'order'
  seller?: string
  orderNumber?: string
  total?: string
  trackingUrl?: string
  deliveryTo?: string
}

// ── helpers (defensive reads) ────────────────────────────────────────────────
type Obj = Record<string, unknown>
const asObj = (v: unknown): Obj => (v && typeof v === 'object' && !Array.isArray(v) ? (v as Obj) : {})
const str = (v: unknown): string | undefined =>
  typeof v === 'string' ? v : typeof v === 'number' ? String(v) : undefined
const typeOf = (o: Obj): string | undefined => {
  const t = o['@type']
  return Array.isArray(t) ? str(t[0]) : str(t)
}
const nameOf = (v: unknown): string | undefined => (typeof v === 'string' ? v : str(asObj(v).name))
function addr(v: unknown): string | undefined {
  if (!v) return undefined
  if (typeof v === 'string') return v
  const a = asObj(v)
  const parts = [a.streetAddress, a.addressLocality, a.postalCode, a.addressRegion, nameOf(a.addressCountry) ?? a.addressCountry]
    .map(str).filter(Boolean)
  return parts.length ? parts.join(', ') : str(a.name)
}
const airport = (v: unknown): Airport =>
  v && typeof v === 'object' ? { code: str(asObj(v).iataCode), name: str(asObj(v).name) } : { name: str(v) }

function nodeToCard(node: SchemaNode): RichCard | null {
  const o = node as Obj
  const t = typeOf(o)
  if (!t) return null
  const base: Base = {
    reservationId: str(o.reservationId ?? o.reservationNumber),
    underName: nameOf(o.underName),
    fromIcs: o._source === 'ics',
  }

  if (t === 'Event' || t.endsWith('Event')) {
    return { kind: 'event', title: str(o.name) ?? 'Événement', start: str(o.startDate), end: str(o.endDate),
      location: nameOf(o.location), address: addr(asObj(o.location).address ?? o.location),
      description: str(o.description), url: str(o.url), organizer: nameOf(o.organizer),
      organizerEmail: str(o.organizerEmail), uid: str(o.uid),
      isInvite: o._invite === true, ...base }
  }
  if (t === 'EventReservation') {
    const e = asObj(o.reservationFor)
    return { kind: 'event', title: nameOf(e) ?? 'Réservation', start: str(e.startDate), end: str(e.endDate),
      location: nameOf(e.location), address: addr(asObj(e.location).address ?? e.location),
      url: str(o.url ?? e.url), ...base }
  }
  if (t === 'FlightReservation') {
    const f = asObj(o.reservationFor)
    const al = asObj(f.airline ?? f.provider)
    return { kind: 'flight', airline: str(al.name), iata: str(al.iataCode), flightNumber: str(f.flightNumber),
      from: { ...airport(f.departureAirport), time: str(f.departureTime), terminal: str(f.departureTerminal), gate: str(f.departureGate) },
      to:   { ...airport(f.arrivalAirport),   time: str(f.arrivalTime),   terminal: str(f.arrivalTerminal) },
      seat: str(asObj(o.airplaneSeat).seatNumber ?? o.airplaneSeat), boardingGroup: str(o.boardingGroup),
      checkinUrl: str(o.checkinUrl), ...base }
  }
  if (t === 'LodgingReservation') {
    const l = asObj(o.reservationFor)
    return { kind: 'lodging', name: nameOf(l), address: addr(l.address), checkin: str(o.checkinTime),
      checkout: str(o.checkoutTime), url: str(o.url ?? l.url), ...base }
  }
  if (t === 'TrainReservation' || t === 'BusReservation') {
    const r = asObj(o.reservationFor)
    return { kind: 'transit', mode: t === 'TrainReservation' ? 'train' : 'bus', operator: nameOf(r.provider),
      number: str(r.trainNumber ?? r.busNumber),
      from: { name: nameOf(r.departureStation ?? r.departureBusStop), time: str(r.departureTime) },
      to:   { name: nameOf(r.arrivalStation ?? r.arrivalBusStop),     time: str(r.arrivalTime) }, ...base }
  }
  if (t === 'FoodEstablishmentReservation') {
    const r = asObj(o.reservationFor)
    return { kind: 'reservation', variant: 'restaurant', name: nameOf(r), address: addr(r.address),
      time: str(o.startTime ?? r.startDate), partySize: Number(o.partySize) || undefined, url: str(o.url), ...base }
  }
  if (t === 'RentalCarReservation') {
    const r = asObj(o.reservationFor)
    return { kind: 'reservation', variant: 'car', name: nameOf(r.rentalCompany ?? r), time: str(o.pickupTime),
      address: addr(asObj(o.pickupLocation).address ?? o.pickupLocation), url: str(o.url), ...base }
  }
  if (t === 'Order') {
    return { kind: 'order', seller: nameOf(o.seller ?? o.merchant), orderNumber: str(o.orderNumber),
      total: str(asObj(o.priceSpecification).price ?? o.totalPrice),
      trackingUrl: str(asObj(o.orderStatus).trackingUrl ?? o.url), ...base }
  }
  if (t === 'ParcelDelivery') {
    return { kind: 'order', seller: nameOf(o.provider), orderNumber: str(o.trackingNumber),
      trackingUrl: str(o.trackingUrl), deliveryTo: addr(o.deliveryAddress), ...base }
  }
  return null
}

/** Map the message's extracted nodes to rich cards (unknown shapes dropped). */
export function cardsFromNodes(nodes: SchemaNode[] | null | undefined): RichCard[] {
  if (!nodes?.length) return []
  const out: RichCard[] = []
  for (const n of nodes) {
    const c = nodeToCard(n)
    if (c) out.push(c)
  }
  return out
}
