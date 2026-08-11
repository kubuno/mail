import { useEffect, useRef, useState } from 'react'

// ── Pull-to-refresh ───────────────────────────────────────────────────────────
// The gesture everyone knows from native mail apps: drag the list past its top
// and it reloads. The rules it has to respect to feel right, and not to fight
// the rest of the UI:
//
//  • it may only start when the list is ALREADY at the top (scrollTop <= 0),
//    otherwise an ordinary scroll would arm it;
//  • the finger moves further than the indicator — a resistance factor — so the
//    pull feels elastic and cannot be triggered by a careless flick;
//  • it must lose to a HORIZONTAL gesture: the rows carry swipe actions, so the
//    drag only counts once it is clearly vertical;
//  • the browser's own overscroll (Chrome Android's native refresh, iOS
//    rubber-band) has to be suppressed while we handle the pull, or both fire.

/** Finger travel needed to arm the refresh. */
const THRESHOLD = 70
/** The indicator moves this fraction of the finger's travel. */
const RESISTANCE = 0.45
/** Cap, so a long drag does not push the list off screen. */
const MAX_PULL = 100

export interface PullState {
  /** Current indicator offset in pixels (0 when idle). */
  offset:     number
  /** Past the threshold: releasing now triggers the refresh. */
  armed:      boolean
  /** The refresh is running. */
  refreshing: boolean
}

/**
 * Wires the gesture onto `ref`'s element. `onRefresh` runs when the user
 * releases past the threshold; the indicator stays out while it resolves.
 * Pass `enabled: false` (desktop) to leave the element untouched.
 */
export function usePullToRefresh(
  ref: React.RefObject<HTMLElement | null>,
  onRefresh: () => Promise<unknown>,
  enabled = true,
): PullState {
  const [offset, setOffset]         = useState(0)
  const [refreshing, setRefreshing] = useState(false)
  // Kept in refs: the handlers are attached once, and reading state from them
  // would capture the values of the render that attached them.
  // Mirror of `offset`: the release handler needs the CURRENT value, and
  // starting the refresh from inside a state updater would run the side effect
  // twice under StrictMode (and leave the indicator stuck out).
  const offsetRef = useRef(0)
  const startY   = useRef(0)
  const startX   = useRef(0)
  const pulling  = useRef(false)
  const decided  = useRef(false)
  const busy     = useRef(false)

  useEffect(() => {
    const el = ref.current
    if (!el || !enabled) return

    const onStart = (e: TouchEvent) => {
      if (busy.current || e.touches.length !== 1 || el.scrollTop > 0) return
      startY.current  = e.touches[0].clientY
      startX.current  = e.touches[0].clientX
      pulling.current = true
      decided.current = false
    }

    const onMove = (e: TouchEvent) => {
      if (!pulling.current) return
      const dy = e.touches[0].clientY - startY.current
      const dx = e.touches[0].clientX - startX.current

      // First movement decides whose gesture this is: a swipe on a row (mostly
      // horizontal) or a pull (downward). Once decided, it does not change.
      if (!decided.current) {
        if (Math.abs(dx) > Math.abs(dy)) { pulling.current = false; return }
        if (dy <= 0) { pulling.current = false; return }
        if (Math.abs(dy) < 6) return
        decided.current = true
      }

      if (dy <= 0 || el.scrollTop > 0) { pulling.current = false; offsetRef.current = 0; setOffset(0); return }
      // Ours now: keep the browser from scrolling or running its own refresh.
      if (e.cancelable) e.preventDefault()
      const next = Math.min(dy * RESISTANCE, MAX_PULL)
      offsetRef.current = next
      setOffset(next)
    }

    const onEnd = () => {
      if (!pulling.current) return
      pulling.current = false
      const reached = offsetRef.current >= THRESHOLD * RESISTANCE
      if (!reached || busy.current) {
        offsetRef.current = 0
        setOffset(0)
        return
      }
      busy.current = true
      setRefreshing(true)
      // Hold the indicator out while the data comes back, then snap home — a
      // refresh that flickers away instantly reads as "nothing happened".
      offsetRef.current = THRESHOLD * RESISTANCE
      setOffset(THRESHOLD * RESISTANCE)
      void Promise.resolve(onRefresh())
        .catch(() => undefined)
        .finally(() => {
          busy.current = false
          setRefreshing(false)
          offsetRef.current = 0
          setOffset(0)
        })
    }

    // `passive: false` on move only: preventDefault is what stops the browser's
    // native overscroll refresh from firing alongside ours.
    el.addEventListener('touchstart', onStart, { passive: true })
    el.addEventListener('touchmove',  onMove,  { passive: false })
    el.addEventListener('touchend',   onEnd,   { passive: true })
    el.addEventListener('touchcancel', onEnd,  { passive: true })
    return () => {
      el.removeEventListener('touchstart', onStart)
      el.removeEventListener('touchmove',  onMove)
      el.removeEventListener('touchend',   onEnd)
      el.removeEventListener('touchcancel', onEnd)
    }
  }, [ref, onRefresh, enabled])

  return { offset, armed: offset >= THRESHOLD * RESISTANCE, refreshing }
}
