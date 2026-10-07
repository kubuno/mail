// Pointer-aware activation (local copy; consolidate into @ui.openable on next
// @kubuno/ui bump). On touch devices double-click is not a natural gesture, so a
// single tap "opens"; on mouse, single click selects and double click opens.

export function isCoarsePointer(): boolean {
  return (
    typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    (window.matchMedia("(pointer: coarse)").matches ||
      window.matchMedia("(hover: none)").matches)
  )
}

type AnyMouseEvent = { stopPropagation(): void; preventDefault(): void }

export function openable<E extends AnyMouseEvent>(opts: {
  open: (e: E) => void
  select?: (e: E) => void
}): { onClick: (e: E) => void; onDoubleClick: (e: E) => void } {
  return {
    onClick: (e) => {
      if (isCoarsePointer()) opts.open(e)
      else opts.select?.(e)
    },
    onDoubleClick: (e) => {
      if (!isCoarsePointer()) opts.open(e)
    },
  }
}
