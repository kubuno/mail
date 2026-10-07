import { useEffect, useRef } from 'react'
import {
  PRIORITY_MARK_FILLED, PRIORITY_MARK_OUTLINE, FOLLOW_STAR_FILLED, FOLLOW_STAR_OUTLINE,
  SELECT_BOX_BLANK, SELECT_BOX_CHECKED, SELECT_BOX_PARTIAL,
} from '../rowMarks'

// Conversation row chrome, measured on Gmail itself: read rows carry the
// blue-grey tint while unread ones stay white, the separator is an inset
// shadow (so it vanishes when the row lifts), and hovering swaps it for a
// two-layer elevation plus 1px side borders — the background stays put.
export const ROW_READ_BG      = '#f2f6fc'
export const ROW_CHECKED_BG   = '#c2dbff'
export const ROW_HIGHLIGHT_BG = '#e8f0fe'
export const ROW_SEPARATOR_SHADOW = 'inset 0 -1px 0 0 rgba(100, 121, 143, 0.12)'
export const ROW_HOVER_SHADOW =
  'inset 1px 0 0 0 #dadce0, inset -1px 0 0 0 #dadce0, ' +
  '0 1px 2px 0 rgba(60, 64, 67, 0.3), 0 1px 3px 1px rgba(60, 64, 67, 0.15)'

/**
 * Gutter marks are dimmed until the row is hovered — an inactive mark sits at
 * 0.32, an active one at 0.7, and hovering restores full strength.
 *
 * Measured on screen: idle the glyphs read luminance ~195 on a white row
 * (0.32 x 68 + 0.68 x 255), hovered they read 70. ⚠️ Verify this with a plain
 * viewport screenshot — `captureBeyondViewport` re-renders the page and loses
 * the hover state, which makes the dimming look like it does not exist.
 */
export function markOpacity(active: boolean, hovered: boolean) {
  if (hovered) return 1
  return active ? 0.7 : 0.32
}

/**
 * Selection box drawn from the shipped artwork. A real (transparent) checkbox
 * stays on top so clicks, keyboard focus and screen readers keep working.
 */
export function SelectBox({ checked, partial = false, onClick, label, opacity = 1 }: {
  checked: boolean
  /** Some but not all items selected — draws the dash, like Gmail. */
  partial?: boolean
  onClick: (e: React.MouseEvent) => void
  label: string
  opacity?: number
}) {
  const inputRef = useRef<HTMLInputElement>(null)
  // `indeterminate` is a property, never an attribute: it has to be assigned.
  useEffect(() => {
    if (inputRef.current) inputRef.current.indeterminate = partial && !checked
  }, [partial, checked])

  return (
    <span className="relative w-5 h-5 flex items-center justify-center">
      <input
        ref={inputRef}
        type="checkbox"
        checked={checked}
        onClick={onClick}
        onChange={() => {}}
        aria-label={label}
        aria-checked={partial && !checked ? 'mixed' : checked}
        className="peer absolute inset-0 w-full h-full m-0 opacity-0 cursor-pointer"
      />
      <img
        src={checked ? SELECT_BOX_CHECKED : partial ? SELECT_BOX_PARTIAL : SELECT_BOX_BLANK}
        width={20}
        height={20}
        alt=""
        aria-hidden="true"
        draggable={false}
        style={{ opacity }}
        className="block select-none pointer-events-none rounded-[2px] transition-opacity
                   peer-focus-visible:ring-2 peer-focus-visible:ring-primary"
      />
    </span>
  )
}

/** Gmail's 2×3 dot drag grip, shown on row hover. */
export function DragGrip() {
  return (
    <svg width="10" height="16" viewBox="0 0 10 16" aria-hidden="true" fill="#80868b">
      {[3, 8, 13].map(cy => (
        <g key={cy}>
          <circle cx="3" cy={cy} r="1.4" />
          <circle cx="7" cy={cy} r="1.4" />
        </g>
      ))}
    </svg>
  )
}

/**
 * Hover action on a row. The slot keeps the 40px pitch (so the icons line up
 * with the date column), but the glyph and its round state layer are smaller:
 * measured on the reference client the drawn ink is only 14–16px, and a 40px
 * pill would run edge to edge in a 40px row, crowding the neighbouring rows.
 * Ink colour matches the gutter marks exactly (#444746).
 */
export function RowAction({ onClick, title, children }: {
  onClick: () => void; title: string; children: React.ReactNode
}) {
  return (
    <button
      onClick={onClick}
      title={title}
      className="w-10 h-10 my-[-10px] flex items-center justify-center group/act"
    >
      <span
        className="w-8 h-8 rounded-full flex items-center justify-center text-[#444746]
                   group-hover/act:bg-black/[0.08] transition-colors"
      >
        {children}
      </span>
    </button>
  )
}

// Gmail-style importance marker (the yellow chevron): filled amber when the
// conversation is important, thin grey outline otherwise.
// List star — Material `star`/`star_border` outline (sharp points, uniform
// thin edge). Lucide's Star has rounded joins and reads noticeably heavier.
export function ListStar({ active, size = 20, opacity = 1 }: {
  active: boolean; size?: number; opacity?: number
}) {
  // Same reasoning as the priority mark: the shipped bitmap, not a redrawn path.
  return (
    <img
      src={active ? FOLLOW_STAR_FILLED : FOLLOW_STAR_OUTLINE}
      width={size}
      height={size}
      alt=""
      aria-hidden="true"
      draggable={false}
      style={{ opacity }}
      className="block select-none transition-opacity"
    />
  )
}

export function ImportanceMarker({ active, opacity = 1 }: { active: boolean; opacity?: number }) {
  // Bitmap artwork rather than a hand-traced path: the outline weight and the
  // notch angles never quite matched when redrawn. Source is 2x, drawn at 20px.
  return (
    <img
      src={active ? PRIORITY_MARK_FILLED : PRIORITY_MARK_OUTLINE}
      width={20}
      height={20}
      alt=""
      aria-hidden="true"
      draggable={false}
      style={{ opacity }}
      className="block select-none transition-opacity"
    />
  )
}
