// @mention wiring shared by the two composers (ComposeWindow, InlineCompose).
//
// The mail body is a plain contenteditable driven by execCommand. This hook
// bolts the core @mention infrastructure onto it: the headless autocomplete
// engine (`useMentionAutocomplete`) fed with the caret context, the portalled
// `MentionList` dropdown, and the chip helpers (styles, insertion, removal).
// A registered provider (Contacts) supplies the candidates at runtime; when no
// provider is registered the feature is silently inert.
//
// The caller owns the body ref and a single `onAfterChange` callback (touch the
// draft autosave + refresh the "logically empty" placeholder). This hook calls
// it after every mutation it makes to the body (chip insert / chip remove) and
// on every input, so the existing placeholder/autosave logic keeps working —
// a lone chip counts as content because its text (+ the × glyph) survives the
// `isLogicallyEmpty` text-content test.

import { useCallback, useEffect, useMemo, useRef, type RefObject } from 'react'
import {
  useMentionAutocomplete,
  MentionList,
  ensureMentionStyles,
  replaceMentionQueryWithChip,
  bindMentionChipRemoval,
  type MentionItem,
  type MentionMatch,
} from '@ui'

// Keys the dropdown owns while it is open; the caret sync must not fire on their
// keyup (it would recompute the query and could reset the active row).
const NAV_KEYS = new Set(['ArrowUp', 'ArrowDown', 'Enter', 'Escape', 'Tab'])

/** Viewport rect of the collapsed caret, with a fallback: a bare collapsed range
 *  can report an empty rect in Chrome, so we measure the character just before
 *  the caret instead. */
function caretRect(range: Range): DOMRect | null {
  const rect = range.getBoundingClientRect()
  if (rect && (rect.width || rect.height || rect.top || rect.left)) return rect
  const node = range.startContainer
  if (node.nodeType === Node.TEXT_NODE && range.startOffset > 0) {
    const probe = document.createRange()
    probe.setStart(node, range.startOffset - 1)
    probe.setEnd(node, range.startOffset)
    const r = probe.getClientRects()[0]
    if (r) return r
  }
  return rect ?? null
}

export function useComposeMentions(
  bodyRef: RefObject<HTMLDivElement | null>,
  onAfterChange: () => void,
) {
  // Keep the latest callback without churning the memoised handlers below.
  const cb = useRef(onAfterChange)
  cb.current = onAfterChange

  const mention = useMentionAutocomplete({
    onSelect: (item: MentionItem, match: MentionMatch) => {
      // Replace the typed `@query` at the caret with a contenteditable=false chip
      // (keeps the native undo stack — see the helper's doc). Then run the same
      // side effects as a normal input so autosave + placeholder stay in sync.
      replaceMentionQueryWithChip(match, item)
      cb.current()
    },
  })

  // Bind chip-removal (delegated click on the ×) and inject the chip CSS onto the
  // CURRENT body element. A callback ref rather than a mount effect so the
  // binding follows the element across unmount/remount (minimize/restore and the
  // windowed ↔ full-screen switch recreate the contenteditable node).
  const cleanupRef = useRef<null | (() => void)>(null)
  const attachBody = useCallback((el: HTMLDivElement | null) => {
    cleanupRef.current?.()
    cleanupRef.current = null
    bodyRef.current = el
    if (el) {
      ensureMentionStyles()
      cleanupRef.current = bindMentionChipRemoval(el, () => cb.current())
    }
  }, [bodyRef])
  // Safety net: tear the binding down if the component itself unmounts.
  useEffect(() => () => { cleanupRef.current?.(); cleanupRef.current = null }, [])

  // Feed the autocomplete the caret context: the current text node's content up
  // to the caret (matches what `replaceMentionQueryWithChip` re-derives) and the
  // caret rect the dropdown anchors to. Anything but a collapsed caret inside the
  // body closes the dropdown.
  const syncCaret = useCallback(() => {
    const el = bodyRef.current
    const sel = window.getSelection()
    if (!el || !sel || sel.rangeCount === 0) return
    const range = sel.getRangeAt(0)
    if (!range.collapsed || !el.contains(range.startContainer)) {
      mention.close()
      return
    }
    const node = range.startContainer
    const textBeforeCaret =
      node.nodeType === Node.TEXT_NODE
        ? (node.textContent ?? '').slice(0, range.startOffset)
        : ''
    mention.handleCaret({ textBeforeCaret, anchorRect: caretRect(range) })
  }, [bodyRef, mention])

  const onInput = useCallback(() => {
    cb.current()
    syncCaret()
  }, [syncCaret])

  const onKeyUp = useCallback(
    (e: React.KeyboardEvent) => {
      if (!NAV_KEYS.has(e.key)) syncCaret()
    },
    [syncCaret],
  )

  const onKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      // Only steals ↑/↓/Enter/Esc/Tab while the dropdown is open (returns false
      // otherwise), so normal typing/newlines/tabbing are untouched.
      if (mention.handleKeyDown(e)) e.preventDefault()
    },
    [mention],
  )

  const onMouseUp = useCallback(() => syncCaret(), [syncCaret])

  // The dropdown sits above the windowed composer natively: @ui's MentionList
  // portals to <body> at z-[9999], above the FloatingWindow layer (windowZStore
  // starts at 1000). No per-consumer z bump needed.

  const mentionList = useMemo(
    () => (
      <MentionList
        items={mention.items}
        activeIndex={mention.activeIndex}
        query={mention.query}
        anchorRect={mention.anchorRect}
        loading={mention.loading}
        onHover={mention.setActiveIndex}
        onPick={item => mention.selectItem(item)}
      />
    ),
    [mention],
  )

  return { attachBody, onInput, onKeyUp, onKeyDown, onMouseUp, mentionList }
}
