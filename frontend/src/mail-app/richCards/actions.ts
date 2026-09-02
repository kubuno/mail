import { ModuleServiceRegistry } from '@kubuno/sdk'
import type { EventCard } from './parse'

/** The calendar module is optional (polyrepo rule): expose its "createEvent"
 *  service only when it's actually loaded, so the button can hide itself. */
export function calendarService():
  | ((input: { title: string; startsAt: string; endsAt?: string; description?: string; location?: string; url?: string; allDay?: boolean }) => Promise<unknown>)
  | undefined {
  return ModuleServiceRegistry.get('calendar', 'createEvent') as
    | ((input: { title: string; startsAt: string; endsAt?: string; description?: string; location?: string; url?: string; allDay?: boolean }) => Promise<unknown>)
    | undefined
}

const allDayOf = (c: EventCard) => !!c.start && !/[T\s]\d{2}:\d{2}/.test(c.start)

/** Push an event card into the user's calendar via the calendar module. */
export async function addEventToCalendar(c: EventCard): Promise<void> {
  const create = calendarService()
  if (!create || !c.start) throw new Error('calendar unavailable')
  await create({
    title:       c.title,
    startsAt:    c.start,
    endsAt:      c.end,
    description: c.description,
    location:    c.location ?? c.address,
    url:         c.url,
    allDay:      allDayOf(c),
  })
}

// ── .ics export ──────────────────────────────────────────────────────────────
const pad = (n: number) => String(n).padStart(2, '0')
/** ISO-ish local/utc string → ICS DTSTART value. A trailing Z stays UTC; a
 *  date-only string becomes VALUE=DATE. */
function icsStamp(iso: string): { value: string; dateOnly: boolean } {
  if (/^\d{4}-\d{2}-\d{2}$/.test(iso)) return { value: iso.replace(/-/g, ''), dateOnly: true }
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return { value: iso.replace(/[-:]/g, ''), dateOnly: false }
  if (iso.endsWith('Z')) {
    const s = `${d.getUTCFullYear()}${pad(d.getUTCMonth() + 1)}${pad(d.getUTCDate())}T${pad(d.getUTCHours())}${pad(d.getUTCMinutes())}${pad(d.getUTCSeconds())}Z`
    return { value: s, dateOnly: false }
  }
  const s = `${d.getFullYear()}${pad(d.getMonth() + 1)}${pad(d.getDate())}T${pad(d.getHours())}${pad(d.getMinutes())}${pad(d.getSeconds())}`
  return { value: s, dateOnly: false }
}
const esc = (v: string) => v.replace(/\\/g, '\\\\').replace(/;/g, '\\;').replace(/,/g, '\\,').replace(/\n/g, '\\n')

/** Build a minimal VCALENDAR for one event and hand it to the browser as a
 *  download — for adding to an external calendar app. */
export function downloadEventIcs(c: EventCard): void {
  if (!c.start) return
  const dt = icsStamp(c.start)
  const suffix = dt.dateOnly ? ';VALUE=DATE' : ''
  const lines = [
    'BEGIN:VCALENDAR', 'VERSION:2.0', 'PRODID:-//Kubuno//Mail//FR', 'BEGIN:VEVENT',
    `UID:${c.reservationId ?? Math.abs(hash(c.title + c.start))}@kubuno`,
    `DTSTART${suffix}:${dt.value}`,
    ...(c.end ? [`DTEND${suffix}:${icsStamp(c.end).value}`] : []),
    `SUMMARY:${esc(c.title)}`,
    ...(c.location ?? c.address ? [`LOCATION:${esc((c.location ?? c.address)!)}`] : []),
    ...(c.description ? [`DESCRIPTION:${esc(c.description)}`] : []),
    ...(c.url ? [`URL:${esc(c.url)}`] : []),
    'END:VEVENT', 'END:VCALENDAR',
  ]
  const blob = new Blob([lines.join('\r\n')], { type: 'text/calendar;charset=utf-8' })
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = `${c.title.replace(/[^\w.-]+/g, '_').slice(0, 40) || 'evenement'}.ics`
  document.body.appendChild(a)
  a.click()
  a.remove()
  setTimeout(() => URL.revokeObjectURL(url), 1000)
}

function hash(s: string): number {
  let h = 0
  for (let i = 0; i < s.length; i++) h = (Math.imul(31, h) + s.charCodeAt(i)) | 0
  return h
}
