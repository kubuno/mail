import { useCallback, useEffect, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { mailApi, EmailAddress } from '../api'
import { logicalBodyText } from './composeContent'

// ── Auto-save a compose/reply as a draft, Gmail-style ─────────────────────────
// The rules that keep it from becoming a mess of stray or duplicated drafts:
//
//  • ONE draft per compose session. The id returned by the first save is reused
//    for every later save (PATCH, never a second POST) — this is exactly the
//    guarantee whose absence gives Gmail users "dozens of duplicate drafts".
//  • Lazy creation: no draft is CREATED until there is REAL content that differs
//    from the state the composer opened with. A reply opens pre-filled with the
//    quoted message and the "Re:" subject; typing nothing then closing must
//    leave no draft behind.
//  • A draft is NEVER auto-deleted. Once it exists, only two things remove it:
//    an explicit discard by the user, and sending the message. Emptying the
//    fields does not delete it — it just keeps the (now empty) draft in sync.
//  • Serialised saves: a promise chain runs one save at a time, so two flushes
//    racing (a debounce firing as the window closes) cannot both POST.
//  • Flush on close: the last edit is saved before the composer unmounts. The
//    body is an uncontrolled contentEditable whose ref is already detached by
//    the time the unmount cleanup runs, so we save from the snapshot captured on
//    the last edit — never a re-read that would persist an empty body.
//  • On send: the draft id is handed to the send call (the backend deletes it),
//    and auto-save is suspended so nothing re-creates it afterwards.

/** The message being composed, read on demand (the body is an uncontrolled
 *  contentEditable, so it cannot live in React state). */
export interface DraftSnapshot {
  account_id:    string
  to_addresses:  EmailAddress[]
  cc_addresses:  EmailAddress[]
  bcc_addresses: EmailAddress[]
  subject:       string
  body_html:     string
  reply_to_id?:  string
}

export type DraftStatus = 'idle' | 'saving' | 'saved'

/** Delay after the last edit before a save fires. */
const DEBOUNCE_MS = 1500

/** Stable string identity of a snapshot, for the "changed since open" test. */
function fingerprint(s: DraftSnapshot): string {
  const addrs = (a: EmailAddress[]) => a.map(x => x.email.toLowerCase()).sort().join(',')
  return JSON.stringify([addrs(s.to_addresses), addrs(s.cc_addresses), addrs(s.bcc_addresses), s.subject, s.body_html])
}

/** Worth persisting = the user actually put something in it. Recipients or a
 *  subject alone qualify; body text alone qualifies. The body is measured with
 *  the auto-inserted signature and the quoted reply/forward EXCLUDED, so a body
 *  holding only those (the state a composer opens in) never counts as content
 *  and never spawns a phantom draft. */
function hasContent(s: DraftSnapshot): boolean {
  return s.to_addresses.length > 0 || s.cc_addresses.length > 0 || s.bcc_addresses.length > 0
      || s.subject.trim() !== '' || logicalBodyText(s.body_html) !== ''
}

export interface DraftAutosave {
  status:  DraftStatus
  /** Call on every edit (recipients, subject, body input): schedules a save. */
  touch:   () => void
  /** Call once, after the composer's initial content is in place, so "changed"
   *  is measured against what the user was given — not against empty. */
  captureBaseline: () => void
  /** Save now (used just before the composer closes). */
  flush:   () => Promise<string | null>
  /** Stop auto-saving and hand back the draft id, to pass as `draft_id` on send
   *  (the backend deletes it once the message actually goes out). */
  markSent: () => string | null
  /** Discard: delete the draft if one exists, and stop saving. */
  discard: () => Promise<void>
  /** The current draft id, or null before the first save. */
  draftId: () => string | null
}

export function useDraftAutosave(getSnapshot: () => DraftSnapshot | null, initialId?: string): DraftAutosave {
  const qc = useQueryClient()
  const [status, setStatus] = useState<DraftStatus>('idle')

  const idRef       = useRef<string | null>(initialId ?? null)
  const baselineRef = useRef<string | null>(null)
  const timerRef    = useRef<ReturnType<typeof setTimeout> | null>(null)
  const chainRef    = useRef<Promise<unknown>>(Promise.resolve())
  const suspended   = useRef(false)
  // The most recent snapshot captured while the composer is still mounted (so
  // the body ref is live). The unmount flush saves from THIS, because by then a
  // fresh getSnapshot() would read a detached ref and lose the body.
  const lastSnapRef = useRef<DraftSnapshot | null>(null)

  const refreshCaches = useCallback(() => {
    qc.invalidateQueries({ queryKey: ['mail-counts'] })
    qc.invalidateQueries({ queryKey: ['mail-drafts'] })
    qc.invalidateQueries({ queryKey: ['mail-threads'] })
  }, [qc])

  // The single save routine, always run through the serialising chain. Pass an
  // explicit snapshot for the unmount flush; otherwise it reads the live one.
  const runSave = useCallback((override?: DraftSnapshot | null): Promise<string | null> => {
    chainRef.current = chainRef.current.then(async () => {
      if (suspended.current) return
      const snap = override ?? getSnapshot()
      if (!snap || !snap.account_id) return

      // Create the draft only once there is real content that differs from what
      // the composer opened with. But once a draft EXISTS, keep it in sync and
      // never delete it here — emptying the fields is not a deletion.
      if (!idRef.current) {
        const changed = fingerprint(snap) !== baselineRef.current
        if (!(hasContent(snap) && changed)) return
      }

      setStatus('saving')
      const dto = {
        account_id:    snap.account_id,
        to_addresses:  snap.to_addresses,
        cc_addresses:  snap.cc_addresses,
        bcc_addresses: snap.bcc_addresses,
        subject:       snap.subject,
        body_html:     snap.body_html,
        reply_to_id:   snap.reply_to_id,
      }
      try {
        if (!idRef.current) {
          const { id } = await mailApi.saveDraft(dto)
          idRef.current = id
        } else {
          await mailApi.updateDraft(idRef.current, dto)
        }
        setStatus('saved')
        refreshCaches()
      } catch {
        // A failed save must not break composing; the next edit retries.
        setStatus('idle')
      }
    })
    return chainRef.current.then(() => idRef.current)
  }, [getSnapshot, refreshCaches])

  const touch = useCallback(() => {
    if (suspended.current) return
    // Capture the live snapshot now (ref still attached), so the unmount flush
    // has the real body even after the contentEditable is torn down.
    lastSnapRef.current = getSnapshot()
    if (timerRef.current) clearTimeout(timerRef.current)
    timerRef.current = setTimeout(() => { void runSave() }, DEBOUNCE_MS)
  }, [runSave, getSnapshot])

  const captureBaseline = useCallback(() => {
    const snap = getSnapshot()
    if (snap) {
      baselineRef.current = fingerprint(snap)
      // Seed the last-good snapshot with the opening content (body ref is live
      // here): closing a resumed draft without edits then re-saves the original,
      // never an empty body read from a detached ref.
      lastSnapRef.current = snap
    }
  }, [getSnapshot])

  const flush = useCallback((): Promise<string | null> => {
    if (timerRef.current) { clearTimeout(timerRef.current); timerRef.current = null }
    return runSave()
  }, [runSave])

  const markSent = useCallback((): string | null => {
    suspended.current = true
    if (timerRef.current) { clearTimeout(timerRef.current); timerRef.current = null }
    return idRef.current
  }, [])

  const discard = useCallback(async (): Promise<void> => {
    suspended.current = true
    if (timerRef.current) { clearTimeout(timerRef.current); timerRef.current = null }
    const id = idRef.current
    idRef.current = null
    if (id) {
      try { await mailApi.deleteDraft(id); refreshCaches() } catch { /* ignore */ }
    }
  }, [refreshCaches])

  // Closing the composer saves the last edit — unless a send/discard already
  // settled the draft's fate. We save from the snapshot captured on the last
  // edit: by now the body's contentEditable is detached, so a fresh read would
  // persist an empty body (and, worse, used to look like "no content").
  useEffect(() => {
    return () => {
      if (suspended.current) return
      if (timerRef.current) clearTimeout(timerRef.current)
      // Fire-and-forget: the component is unmounting, so we cannot await, but the
      // request is already on the wire.
      void runSave(lastSnapRef.current)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  return {
    status, touch, captureBaseline, flush, markSent, discard,
    draftId: () => idRef.current,
  }
}
