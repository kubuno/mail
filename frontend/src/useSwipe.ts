import { useRef, useState, useCallback } from 'react'
import type { TouchEvent as ReactTouchEvent } from 'react'

function isCoarse(): boolean {
  return (
    typeof window !== 'undefined' &&
    typeof window.matchMedia === 'function' &&
    (window.matchMedia('(pointer: coarse)').matches || window.matchMedia('(hover: none)').matches)
  )
}

/**
 * Swipe-to-action for list rows (Gmail-style). Returns the live horizontal
 * offset `dx` (apply as translateX) plus touch handlers. Combine with
 * `touch-action: pan-y` on the row so vertical scrolling keeps working while
 * horizontal swipes are captured. Fires onRight when swiped right past the
 * threshold, onLeft when swiped left past it.
 */
export function useSwipeActions(opts: { onLeft?: () => void; onRight?: () => void; threshold?: number }) {
  const { onLeft, onRight, threshold = 80 } = opts
  const [dx, setDx] = useState(0)
  const [swiping, setSwiping] = useState(false)
  const start = useRef<{ x: number; y: number } | null>(null)
  const active = useRef(false)

  const onTouchStart = useCallback((e: ReactTouchEvent) => {
    if (!isCoarse() || e.touches.length !== 1) return
    start.current = { x: e.touches[0].clientX, y: e.touches[0].clientY }
    active.current = false
  }, [])

  const onTouchMove = useCallback((e: ReactTouchEvent) => {
    if (!start.current) return
    const dxRaw = e.touches[0].clientX - start.current.x
    const dyRaw = e.touches[0].clientY - start.current.y
    if (!active.current) {
      // Engage only on a clearly horizontal gesture; bail on vertical (scroll).
      if (Math.abs(dxRaw) > 10 && Math.abs(dxRaw) > Math.abs(dyRaw) * 1.5) {
        active.current = true
        setSwiping(true)
      } else if (Math.abs(dyRaw) > 10) {
        start.current = null
        return
      } else return
    }
    setDx(Math.max(-140, Math.min(140, dxRaw)))
  }, [])

  const onTouchEnd = useCallback(() => {
    if (active.current) {
      if (dx <= -threshold && onLeft) onLeft()
      else if (dx >= threshold && onRight) onRight()
    }
    start.current = null
    active.current = false
    setSwiping(false)
    setDx(0)
  }, [dx, onLeft, onRight, threshold])

  return { dx, swiping, handlers: { onTouchStart, onTouchMove, onTouchEnd, onTouchCancel: onTouchEnd } }
}
