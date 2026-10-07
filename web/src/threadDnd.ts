/** Payload carried while dragging conversations from the list. */
export const THREAD_DND_TYPE = 'application/x-kubuno-mail-thread'

/**
 * Drag feedback, the way Gmail does it.
 *
 * `setDragImage` is deliberately NOT used for the visible pill: the browser
 * snapshots the node and renders it translucent, which looks washed out. So we
 * hand the browser a 1×1 transparent pixel to suppress the native ghost, and
 * draw our own fully opaque pill that follows the cursor — which also lets us
 * show the hovered target's name above it.
 */
const TRANSPARENT_PIXEL =
  'data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7'

let ghost:   HTMLDivElement | null = null
let caption: HTMLDivElement | null = null

function buildGhost(label: string) {
  const root = document.createElement('div')
  root.style.cssText = [
    'position:fixed', 'top:0', 'left:0', 'z-index:2147483647',
    'pointer-events:none', 'will-change:transform',
    'display:flex', 'flex-direction:column', 'align-items:flex-start', 'gap:6px',
  ].join(';')

  // Target name, shown only while hovering a drop zone.
  caption = document.createElement('div')
  caption.style.cssText = [
    'display:none', 'margin-left:24px',
    'padding:6px 10px', 'border-radius:4px',
    'background:#3c4043', 'color:#fff',
    'font:400 13px/1 system-ui, -apple-system, Segoe UI, Roboto, sans-serif',
    'white-space:nowrap', 'box-shadow:0 1px 3px rgba(60,64,67,.3)',
  ].join(';')

  const pill = document.createElement('div')
  pill.style.cssText = [
    'display:flex', 'align-items:center', 'gap:12px',
    'padding:14px 20px', 'border-radius:8px',
    'background:#1a73e8', 'color:#fff', 'opacity:1',
    'font:600 15px/1 system-ui, -apple-system, Segoe UI, Roboto, sans-serif',
    'white-space:nowrap', 'box-shadow:0 1px 3px rgba(60,64,67,.3), 0 4px 8px 3px rgba(60,64,67,.15)',
  ].join(';')
  pill.innerHTML =
    '<svg width="20" height="20" viewBox="0 0 24 24" fill="#fff" aria-hidden="true">' +
    '<path d="M20 4H4a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2V6a2 2 0 0 0-2-2zm0 4-8 5-8-5V6l8 5 8-5z"/>' +
    '</svg>'
  pill.appendChild(document.createTextNode(label))

  root.appendChild(caption)
  root.appendChild(pill)
  return root
}

function moveGhost(x: number, y: number) {
  if (!ghost || (x === 0 && y === 0)) return
  ghost.style.transform = `translate(${x + 14}px, ${y + 14}px)`
}

function onDragOver(e: DragEvent) { moveGhost(e.clientX, e.clientY) }
function onDrag(e: DragEvent)     { moveGhost(e.clientX, e.clientY) }

function endDrag() {
  ghost?.remove()
  ghost = null
  caption = null
  document.removeEventListener('dragover', onDragOver, true)
  document.removeEventListener('drag',     onDrag,     true)
  document.removeEventListener('dragend',  endDrag,    true)
  document.removeEventListener('drop',     endDrag,    true)
}

/** Call from a row's `onDragStart`. Returns nothing; cleans itself up. */
export function startThreadDrag(e: React.DragEvent, ids: string[], label: string) {
  e.dataTransfer.setData(THREAD_DND_TYPE, ids.join(','))
  e.dataTransfer.effectAllowed = 'move'

  const pixel = new Image()
  pixel.src = TRANSPARENT_PIXEL
  e.dataTransfer.setDragImage(pixel, 0, 0)

  endDrag()
  ghost = buildGhost(label)
  document.body.appendChild(ghost)
  moveGhost(e.clientX, e.clientY)

  // Capture phase, so a zone calling stopPropagation cannot strand the ghost.
  document.addEventListener('dragover', onDragOver, true)
  document.addEventListener('drag',     onDrag,     true)
  document.addEventListener('dragend',  endDrag,    true)
  document.addEventListener('drop',     endDrag,    true)
}

/** Name of the zone under the cursor, shown above the pill (null hides it). */
export function setThreadDropCaption(name: string | null) {
  if (!caption) return
  caption.textContent = name ?? ''
  caption.style.display = name ? 'block' : 'none'
}

/** Reads the dragged conversation ids back out of a drop event. */
export function readDraggedThreads(e: React.DragEvent): string[] {
  return (e.dataTransfer.getData(THREAD_DND_TYPE) || '')
    .split(',')
    .map(s => s.trim())
    .filter(Boolean)
}

/** True when the drag carries conversations (so a zone may accept the drop). */
export function isThreadDrag(e: React.DragEvent) {
  return e.dataTransfer.types.includes(THREAD_DND_TYPE)
}
