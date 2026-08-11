// Compose body helpers shared by the composers (ComposeWindow, InlineCompose).
//
// Gmail-style "logically empty" body: the placeholder ("Rédigez votre message…")
// stays visible while the body holds nothing but the auto-inserted signature
// and/or the quoted message it replies to/forwards. Those two blocks are wrapped
// in marked containers so we can strip them and inspect what the user actually
// typed.

/** Attribute marking the auto-inserted signature block. */
export const SIGNATURE_ATTR = 'data-kb-signature'
/** Attribute marking the reply/forward quote block. */
export const QUOTE_ATTR = 'data-kb-quote'

/** Wrap a signature's HTML in a marked, removable container (with a leading gap,
 *  matching the menu insertion). */
export function signatureBlock(html: string): string {
  return `<div ${SIGNATURE_ATTR}><br><br>${html}</div>`
}

/** The body's text with the signature and quote blocks removed — the user's own
 *  content. Empty (after trimming whitespace/&nbsp;/<br>) means "logically empty":
 *  the placeholder should show and no draft should be created. */
export function logicalBodyText(html: string): string {
  if (!html) return ''
  const tpl = document.createElement('template')
  tpl.innerHTML = html
  tpl.content.querySelectorAll(`[${SIGNATURE_ATTR}],[${QUOTE_ATTR}]`).forEach(n => n.remove())
  // \s already covers &nbsp; (U+00A0) in the JS regex engine.
  return (tpl.content.textContent ?? '').replace(/\s+/g, ' ').trim()
}

/** True when the body carries no user-typed content (only signature/quote). */
export function isLogicallyEmpty(html: string): boolean {
  return logicalBodyText(html) === ''
}

const fmtSize = (n: number) =>
  n < 1024 ? `${n} o` : n < 1048576 ? `${Math.round(n / 1024)} Ko` : `${(n / 1048576).toFixed(1)} Mo`

const escapeHtml = (s: string) =>
  s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;')

/** Append a Gmail-style block of Drive share links to the message body at SEND
 *  time (oversized files travel as public links, not base64). Each file is one
 *  bordered card: a Drive glyph, the file name linking to its public URL, and
 *  its size. Returns `html` unchanged when there are no links. Injected only at
 *  send so it never disturbs the live editor, its undo stack or the draft. */
export function appendDriveLinksHtml(
  html: string,
  links: { filename: string; size: number; url: string }[],
  intro: string,
): string {
  if (!links.length) return html
  const cards = links.map(l =>
    `<a href="${escapeHtml(l.url)}" style="display:flex;align-items:center;gap:10px;text-decoration:none;` +
    `border:1px solid #dadce0;border-radius:8px;padding:8px 12px;margin:6px 0;max-width:400px;color:#202124">` +
    `<span style="font-size:18px">📎</span>` +
    `<span style="min-width:0"><span style="color:#1a73e8;word-break:break-all">${escapeHtml(l.filename)}</span>` +
    `<span style="color:#5f6368;font-size:12px"> (${escapeHtml(fmtSize(l.size))})</span></span></a>`,
  ).join('')
  return `${html}<div data-kb-drive-links style="margin-top:12px">` +
    `<div style="color:#5f6368;font-size:12px;margin-bottom:4px">${escapeHtml(intro)}</div>${cards}</div>`
}

/** Collapse the selection to the very start of the editable body, so the user
 *  types ABOVE the signature/quote (Gmail behaviour). */
export function placeCaretAtStart(el: HTMLElement): void {
  el.focus()
  const sel = window.getSelection()
  if (!sel) return
  const r = document.createRange()
  r.setStart(el, 0)
  r.collapse(true)
  sel.removeAllRanges()
  sel.addRange(r)
}
