import { useEffect, useRef } from 'react'
import DOMPurify from 'dompurify'
import { longDateTimeFromFrench } from './helpers'

// ── Email HTML viewer (Shadow DOM, NO iframe) ─────────────────────────────────
// Strategy to apply the email CSS COMPLETELY, without interference:
//  1. Shadow DOM → the email styles do NOT leak into the app (encapsulation).
//  2. `:host { all: initial }` → cuts the INHERITANCE of the app styles (font,
//     colour, line-height, text-align…) which, unlike the rest, does cross the
//     shadow boundary. That is the "reset on most properties" which keeps the
//     interference out.
//  3. We KEEP the email <body> element (not only its content) so that its
//     `body { … }` rules and its inline style / bgcolor really do apply.
//  4. DOMPurify neutralises scripts, event handlers and dangerous protocols,
//     while PRESERVING <style> and the `style` attributes (the CSS).

// CSS hazards the server also strips, repeated here on purpose: messages stored
// before the server policy was hardened were cleaned by the OLD rules, and this
// is the last pass before the markup reaches a live document. `@import` fetches
// a remote stylesheet (it leaks the read and injects unreviewed rules);
// `expression()`, `behavior:` and `-moz-binding` are the legacy script-in-CSS
// hooks. The whole declaration goes, not just the keyword.
const CSS_HAZARDS = ['@import', 'expression(', 'behavior:', '-moz-binding']
function cleanCss(css: string): string {
  let out = ''
  let cursor = 0
  const lower = css.toLowerCase()
  for (;;) {
    const hits = CSS_HAZARDS.map(h => lower.indexOf(h, cursor)).filter(i => i >= 0)
    if (!hits.length) return out + css.slice(cursor)
    const pos = Math.min(...hits)
    // Back up to the start of the declaration / at-rule holding it…
    const before = Math.max(css.lastIndexOf(';', pos), css.lastIndexOf('{', pos), css.lastIndexOf('}', pos))
    out += css.slice(cursor, before >= cursor ? before + 1 : cursor)
    // …and resume past its end.
    const semi = css.indexOf(';', pos)
    const brace = css.indexOf('}', pos)
    const ends = [semi, brace].filter(i => i >= 0)
    cursor = ends.length ? Math.min(...ends) + 1 : css.length
  }
}

// Sanitising: keep all the CSS, strip only what is dangerous.
//
// This runs on markup the SERVER already sanitised. It is not redundant: a
// message stored months ago carries whatever the policy of the day allowed, and
// only this pass stands between it and a live DOM. Kept deliberately in step
// with services::html_sanitize on the Rust side.
/** Exported so every path that injects e-mail HTML — the reader here, and the
 *  reply/forward quote in the composer — goes through the SAME policy. A caller
 *  reaching for a bare `DOMPurify.sanitize()` would silently get the library's
 *  defaults instead of ours. */
export function sanitizeEmailHtml(html: string): string {
  const clean = DOMPurify.sanitize(html, {
    WHOLE_DOCUMENT: true,                 // keeps <html>/<head>/<body> + the head <style>
    ADD_TAGS: ['style'],
    // Anything that executes, navigates, submits or pulls a remote document.
    FORBID_TAGS: ['script', 'iframe', 'object', 'embed', 'base', 'meta', 'link', 'form',
                  'input', 'button', 'select', 'textarea', 'applet', 'frame', 'frameset',
                  'svg', 'math', 'template', 'noscript', 'portal'],
    FORBID_ATTR: ['ping', 'srcdoc', 'formaction', 'http-equiv'],
    ALLOW_UNKNOWN_PROTOCOLS: false,
    // `data:` stays legitimate for inline images only; the hook below drops it
    // everywhere else, so a link cannot carry an inline HTML document.
    ADD_DATA_URI_TAGS: ['img'],
  })
  return clean
}

// One hook, installed once: it is what enforces the two rules DOMPurify's
// options cannot express — inline schemes are image sources only, and e-mail
// CSS is scrubbed wherever it hides.
let hookInstalled = false
function installSanitizeHook() {
  if (hookInstalled) return
  hookInstalled = true
  DOMPurify.addHook('afterSanitizeAttributes', node => {
    const el = node as Element
    for (const attr of ['href', 'src', 'xlink:href', 'action', 'background']) {
      const value = el.getAttribute?.(attr)
      if (!value) continue
      const v = value.trim().toLowerCase()
      if (v.startsWith('data:') || v.startsWith('cid:')) {
        const inlineImage = el.tagName === 'IMG' && attr === 'src'
          && (v.startsWith('cid:') || v.startsWith('data:image/'))
        if (!inlineImage) el.removeAttribute(attr)
      }
    }
    const style = el.getAttribute?.('style')
    if (style) el.setAttribute('style', cleanCss(style))
    if (el.tagName === 'STYLE' && el.textContent) el.textContent = cleanCss(el.textContent)
  })
}
installSanitizeHook()

// ── Remote images ────────────────────────────────────────────────────────────
// Loading a remote image tells its host the message was opened (and leaks the
// reader's IP), which is why Gmail asks before displaying images from a sender
// you have not trusted yet. We neutralise every URL that would leave the
// instance — and ONLY those: `cid:` (the message's own inline parts), `data:`
// and same-origin URLs carry nothing outward, so blocking them would break
// signatures and attachments for no privacy gain.
//
// This MUST run while the body is still detached from the document: once it is
// appended to the shadow root the browser has already started fetching.

/** True when this URL would make the browser reach outside the instance. */
function isRemoteUrl(raw: string): boolean {
  const u = raw.trim()
  if (!u || u.startsWith('cid:') || u.startsWith('data:') || u.startsWith('#')) return false
  if (/^https?:\/\//i.test(u)) {
    try { return new URL(u).origin !== window.location.origin } catch { return true }
  }
  return /^\/\//.test(u) // protocol-relative → remote
}

/** 1×1 transparent GIF: keeps the layout without asking anyone for bytes. */
const BLANK_PIXEL = 'data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7'

/** Strips remote `url(...)` references from a CSS text (style sheets and inline
 *  style attributes both carry background images). */
function stripRemoteCssUrls(css: string): string {
  return css.replace(/url\(\s*(['"]?)([^)'"]+)\1\s*\)/gi, (whole, _q, url: string) =>
    isRemoteUrl(url) ? 'none' : whole)
}

/** Replaces every remote image reference in a detached email body with a blank
 *  pixel, remembering the original in `data-kb-src` so it can be restored.
 *  Returns how many were held back. */
function blockRemoteImagesIn(body: HTMLElement, styles: HTMLStyleElement[]): number {
  let blocked = 0
  for (const img of Array.from(body.querySelectorAll('img'))) {
    const src = img.getAttribute('src')
    if (src && isRemoteUrl(src)) {
      img.setAttribute('data-kb-src', src)
      img.setAttribute('src', BLANK_PIXEL)
      blocked++
    }
    // `srcset` would defeat a blocked `src` on its own.
    const srcset = img.getAttribute('srcset')
    if (srcset && srcset.split(',').some(c => isRemoteUrl(c.trim().split(/\s+/)[0] ?? ''))) {
      img.setAttribute('data-kb-srcset', srcset)
      img.removeAttribute('srcset')
      blocked++
    }
  }
  // Legacy `background="…"` attribute and CSS backgrounds in inline styles.
  for (const el of Array.from(body.querySelectorAll<HTMLElement>('[background]'))) {
    const bg = el.getAttribute('background') ?? ''
    if (isRemoteUrl(bg)) {
      el.setAttribute('data-kb-background', bg)
      el.removeAttribute('background')
      blocked++
    }
  }
  for (const el of Array.from(body.querySelectorAll<HTMLElement>('[style*="url("]'))) {
    const style = el.getAttribute('style') ?? ''
    const stripped = stripRemoteCssUrls(style)
    if (stripped !== style) {
      el.setAttribute('data-kb-style', style)
      el.setAttribute('style', stripped)
      blocked++
    }
  }
  // …and in the email's own stylesheets.
  for (const st of styles) {
    const css = st.textContent ?? ''
    const stripped = stripRemoteCssUrls(css)
    if (stripped !== css) {
      st.setAttribute('data-kb-css', css)
      st.textContent = stripped
      blocked++
    }
  }
  return blocked
}

/** Counts what WOULD be blocked, without touching anything — so the banner can
 *  stay silent on a message that has no remote image at all. */
function countRemoteImages(body: HTMLElement, styles: HTMLStyleElement[]): number {
  const probe = body.cloneNode(true) as HTMLElement
  const probeStyles = styles.map(st => st.cloneNode(true) as HTMLStyleElement)
  return blockRemoteImagesIn(probe, probeStyles)
}

// Builds the email DOM nodes: the <style> elements (head + body) and the FULL
// <body> element. We return real nodes (not a string) because injecting `<body>`
// through innerHTML would have it REMOVED by the fragment parser (html/head/body
// are only inserted in a document context). By APPENDING the <body> element it
// survives, so the email `body { … }` selectors — very common — really do apply.
function buildEmailNodes(html: string): { styles: HTMLStyleElement[]; body: HTMLElement } {
  const clean = sanitizeEmailHtml(html)
  const isDoc = /<html[\s>]/i.test(clean) || /^\s*<!doctype/i.test(clean)
  const doc = new DOMParser().parseFromString(isDoc ? clean : `<body>${clean}</body>`, 'text/html')

  // <style> of the whole document (some clients put them in the body) — detached
  // so they can be re-inserted at the top of the shadow (before the <body>),
  // without duplicates.
  const styles = Array.from(doc.querySelectorAll('style'))
  styles.forEach(s => s.remove())
  return { styles, body: doc.body }
}

// An attribution line — "Le … a écrit :", "On … wrote:", "-----Original
// Message-----" — introduces a quoted reply. Matching it (rather than any
// blockquote) tells a genuine quote trail apart from a newsletter that merely
// styles text with <blockquote>.
function isAttributionLine(text: string): boolean {
  const t = text.trim()
  if (!t || t.length > 300) return false
  return (
    /\ba\s+écrit\s*:/i.test(t) ||
    /\bwrote:/i.test(t) ||
    /-{2,}\s*(original message|message d'origine|forwarded message|message transféré)/i.test(t)
  )
}

// Text held DIRECTLY by an element (its own text nodes), so a container is not
// mistaken for an attribution line just because one sits deep inside it.
function directText(el: Element): string {
  let s = ''
  el.childNodes.forEach(n => { if (n.nodeType === 3) s += n.nodeValue ?? '' })
  return s
}

const BLOCK_TAGS = new Set(['P', 'DIV', 'TD', 'TH', 'LI', 'BLOCKQUOTE', 'SECTION', 'ARTICLE', 'TABLE', 'TR'])

// Nearest block-level ancestor of `node` within `body` (or null).
function blockAncestor(node: Node, body: HTMLElement): Element | null {
  let el: Element | null = node.nodeType === 1 ? (node as Element) : node.parentElement
  while (el && el !== body && !BLOCK_TAGS.has(el.tagName)) el = el.parentElement
  return el && el !== body ? el : null
}

// The metadata HEADER of a forwarded/quoted message — the De/From, Envoyé/Date,
// À/To, Objet/Subject block that both Outlook ("De: … Objet:") and Gmail
// ("---------- Forwarded message ----------/ De:/ Date:/ Subject:/ To:", inside
// .gmail_attr) emit. Returns the node to fold AFTER, so the WHOLE header stays
// visible and only the quoted body below it folds — what Gmail shows. Handles
// non-breaking spaces ("De :") and headers wrapped in a .gmail_quote.
/** What the collapsed quote is about, so a header can always be shown above the
 *  "•••" even when the quoted mail carries none (our own replies only emit a
 *  "Le … a écrit :" line). Supplied by the card from the message being read. */
export interface QuoteContext {
  /** Recipient of the quoted mail = the author of the message being read. */
  to?:      string
  /** Thread subject, used as the quoted mail's subject. */
  subject?: string
  /** UI language, so a quoted date reads "8 août 2026 à 18h08". */
  lang?:    string
  /** Phone: keep the rebuilt header at body size (13px would be too small). */
  compact?: boolean
}

// "Le <date>, <sender> a écrit :" / "On <date>, <sender> wrote:" → its parts, so
// a De/Envoyé header can be rebuilt from a plain attribution line.
function parseAttribution(text: string): { from: string; sent: string } | null {
  const t = text.replace(/\s+/g, ' ').trim()
  const m = t.match(/\bLe\s+(.+?),\s*(.+?)\s+a\s+écrit\s*:/i) || t.match(/\bOn\s+(.+?),\s*(.+?)\s+wrote\s*:/i)
  return m ? { sent: m[1].trim(), from: m[2].trim() } : null
}


// Appends `text` to `parent`, turning every e-mail address into a mailto link —
// the rebuilt header must read like the ones real clients emit, where addresses
// are clickable. Colour comes from the sheet's `a { … }` rule (theme blue).
function appendLinkified(parent: HTMLElement, text: string) {
  const re = /[\w.!#$%&'*+/=?^`{|}~-]+@[\w-]+(?:\.[\w-]+)+/g
  let last = 0
  for (const m of text.matchAll(re)) {
    const at = m.index ?? 0
    if (at > last) parent.appendChild(document.createTextNode(text.slice(last, at)))
    const a = document.createElement('a')
    a.href = `mailto:${m[0]}`
    a.textContent = m[0]
    parent.appendChild(a)
    last = at + m[0].length
  }
  if (last < text.length) parent.appendChild(document.createTextNode(text.slice(last)))
}

// The De/Envoyé/À/Objet block shown above the "•••". Mirrors what Outlook emits
// (bold labels, one line each) so a reply reads like a forward does.
function buildHeaderBlock(
  info: { from?: string; sent?: string; to?: string; subject?: string },
  compact = false,
): HTMLElement | null {
  const rows: [string, string][] = []
  if (info.from)    rows.push(['De :', info.from])
  if (info.sent)    rows.push(['Envoyé :', info.sent])
  if (info.to)      rows.push(['À :', info.to])
  if (info.subject) rows.push(['Objet :', info.subject])
  if (!rows.length) return null

  const div = document.createElement('div')
  div.style.margin = '10px 0 2px'
  // Slightly smaller than the body, with room to breathe: four dense header
  // lines at body size read as a wall. Left alone on phones, where 13px would
  // be too small and the lines already wrap.
  if (!compact) {
    div.style.fontSize   = '13px'
    div.style.lineHeight = '1.9'
  }
  rows.forEach(([label, value], i) => {
    const b = document.createElement('b')
    b.textContent = label + ' '
    div.appendChild(b)
    appendLinkified(div, value)
    if (i < rows.length - 1) div.appendChild(document.createElement('br'))
  })
  return div
}

// Element text with one line PER TEXT NODE. `textContent` glues <br>-separated
// lines together ("…16:57Subject: AXA"), which hides the start of a header line.
function textLines(el: Element): string {
  const out: string[] = []
  const w = document.createTreeWalker(el, NodeFilter.SHOW_TEXT)
  while (w.nextNode()) out.push(w.currentNode.nodeValue ?? '')
  return out.join('\n')
}

function findForwardHeaderEnd(body: HTMLElement): Node | null {
  const DE   = /(^|[\s>])(De|From)\s*:/i
  const SUBJ = /(^|[\s>])(Objet|Subject)\s*:/i
  const KW   = /(^|[\s>])(De|From|Envoy[eé]|Sent|Date|À|A|To|Cc|Copie|Objet|Subject)\s*:/i

  // The block holding the "De:/From:" line…
  const walk = document.createTreeWalker(body, NodeFilter.SHOW_TEXT)
  let deNode: Text | null = null
  while (walk.nextNode()) {
    if (DE.test(walk.currentNode.nodeValue ?? '')) { deNode = walk.currentNode as Text; break }
  }
  if (!deNode) return null
  const block = blockAncestor(deNode, body)
  // …only if that same block also carries the Subject/Objet line is it truly a
  // forwarded header (guards against a stray "De :" in ordinary prose).
  // Compare on LINES, not textContent: <br>-separated lines are concatenated by
  // textContent ("…16:57Subject: AXA"), so a line-start test would fail there.
  if (!block || !SUBJ.test(textLines(block))) return null

  // Last header line inside the block (Objet/Subject for Outlook; To/Cc for Gmail).
  const inner = document.createTreeWalker(block, NodeFilter.SHOW_TEXT)
  let lastKw: Node = deNode
  while (inner.nextNode()) {
    if (KW.test(inner.currentNode.nodeValue ?? '')) lastKw = inner.currentNode
  }

  // Fold after the <br> ending that line if header and body share the block, else
  // after the whole header block (the body is a following sibling).
  const brWalk = document.createTreeWalker(block, NodeFilter.SHOW_ELEMENT, {
    acceptNode: (n) => (n as Element).tagName === 'BR' ? NodeFilter.FILTER_ACCEPT : NodeFilter.FILTER_SKIP,
  })
  while (brWalk.nextNode()) {
    if (lastKw.compareDocumentPosition(brWalk.currentNode) & Node.DOCUMENT_POSITION_FOLLOWING) {
      return brWalk.currentNode
    }
  }
  return block
}

// Where the collapsible quote begins. `mode:'before'` folds the marker too;
// `mode:'after'` keeps it visible and folds what follows.
function findQuoteBoundary(body: HTMLElement): { node: Node; mode: 'before' | 'after' } | null {
  // 1. A forwarded header (Outlook OR Gmail) → KEEP it, fold the body under it.
  //    Checked BEFORE .gmail_quote so a forward inside a gmail_quote keeps its
  //    De/Date/Subject/To header on screen instead of folding the whole block.
  const objEnd = findForwardHeaderEnd(body)
  if (objEnd) return { node: objEnd, mode: 'after' }

  // 2. A .gmail_quote with no forwarded header = a reply ("On … wrote:") → fold it whole.
  const gq = body.querySelector('.gmail_quote')
  if (gq) return { node: gq, mode: 'before' }

  // 3. Attribution at top level (our own replies: new content, then
  //    "<div>Le … a écrit :</div><blockquote>").
  for (const n of Array.from(body.childNodes)) {
    if (isAttributionLine(n.textContent ?? '')) return { node: n, mode: 'before' }
  }

  // 4. Attribution nested a level down (replies synced from other clients).
  for (const el of Array.from(body.querySelectorAll('div,p,td,blockquote'))) {
    if (isAttributionLine(directText(el))) return { node: el, mode: 'before' }
  }

  return null
}

// Collapses the quoted history behind a Gmail-style "•••" toggle. Uses a DOM
// Range so the cut is clean whatever the nesting — the header and the quoted
// body may sit at different depths, which top-level slicing cannot separate.
function installQuoteToggle(body: HTMLElement, ctx?: QuoteContext) {
  const boundary = findQuoteBoundary(body)
  if (!boundary) return

  // A quote with no header of its own (a reply: just "Le … a écrit :") gets one
  // rebuilt above the toggle, so the "•••" is ALWAYS preceded by the
  // De/Envoyé/À/Objet block — the forwarded case already carries its own.
  const attribution = boundary.mode === 'before'
    ? parseAttribution(boundary.node.textContent ?? '')
    : null
  const synthHeader = boundary.mode === 'before'
    ? buildHeaderBlock({
        from:    attribution?.from,
        // Older quotes carry "08/08/2026 18:08:31"; show the spelled-out form.
        sent:    attribution ? longDateTimeFromFrench(attribution.sent, ctx?.lang ?? 'fr') : undefined,
        to:      ctx?.to,
        subject: ctx?.subject,
      }, !!ctx?.compact)
    : null

  // Fold start. For a "before" fold, swallow the leading <br>/blank siblings so
  // the toggle hugs the content.
  let startNode: Node = boundary.node
  if (boundary.mode === 'before') {
    let p = startNode.previousSibling
    while (p && ((p.nodeType === 1 && (p as Element).tagName === 'BR') || (p.textContent ?? '').trim() === '')) {
      startNode = p
      p = p.previousSibling
    }
  }

  const range = document.createRange()
  if (boundary.mode === 'before') range.setStartBefore(startNode)
  else range.setStartAfter(boundary.node)
  range.setEnd(body, body.childNodes.length)

  // Must be genuine content ABOVE the fold (a message that is ONLY a quote stays
  // fully visible), and something meaningful to fold.
  const above = document.createRange()
  above.setStart(body, 0)
  above.setEnd(range.startContainer, range.startOffset)
  const aboveFrag = above.cloneContents()
  if ((aboveFrag.textContent ?? '').trim() === '' && !aboveFrag.querySelector('img')) return

  const foldClone = range.cloneContents()
  if ((foldClone.textContent ?? '').trim() === '' && !foldClone.querySelector('img')) return

  const wrapper = document.createElement('div')
  wrapper.style.display = 'none'
  wrapper.appendChild(range.extractContents())
  range.insertNode(wrapper)

  const btn = document.createElement('button')
  btn.type = 'button'
  btn.title = 'Afficher le contenu abrégé'
  btn.setAttribute('aria-label', 'Afficher le contenu abrégé')
  btn.setAttribute('aria-expanded', 'false')
  btn.textContent = '•••'
  Object.assign(btn.style, {
    display: 'inline-block', margin: '6px 0', padding: '1px 8px', height: '20px',
    lineHeight: '15px', background: '#e8eaed', border: 'none', borderRadius: '10px',
    color: '#3c4043', cursor: 'pointer', fontSize: '15px', letterSpacing: '1px',
    verticalAlign: 'middle',
  })
  btn.addEventListener('mouseenter', () => { btn.style.background = '#dadce0' })
  btn.addEventListener('mouseleave', () => { btn.style.background = '#e8eaed' })
  btn.addEventListener('click', () => {
    const open = wrapper.style.display !== 'none'
    wrapper.style.display = open ? 'none' : 'block'
    btn.setAttribute('aria-expanded', String(!open))
  })
  wrapper.parentNode?.insertBefore(btn, wrapper)
  // The rebuilt header goes ABOVE the toggle: block first, then the "•••".
  if (synthHeader) wrapper.parentNode?.insertBefore(synthHeader, btn)
}

// Base CSS injected BEFORE the email CSS (the email wins through the cascade).
const BASE_CSS = `
  /* (2) Radical reset: neutralises the inheritance of the app styles inside the
     shadow. all:initial does NOT touch direction/unicode-bidi (spec exclusion). */
  :host {
    all: initial;
    display: block;
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Arial, sans-serif;
    font-size: 14px;
    line-height: 1.6;
    color: #202124;
    text-align: left;
    background: #ffffff;
    padding: 16px;
    /* Fixed-width email layouts (600px tables…) must scroll INSIDE the message,
       never widen the app layout (phones would end up horizontally scrolled). */
    max-width: 100%;
    overflow-x: auto;
  }

  *, *::before, *::after { box-sizing: border-box; }

  img { max-width: 100%; height: auto; }

  /* The email <body> is kept: default margins/typography, the email may redefine. */
  body {
    margin: 0;
    overflow-wrap: break-word;
    font-family: inherit;
    font-size: inherit;
    line-height: inherit;
    color: inherit;
  }
  p, div, span, td, th, table, ul, ol, li,
  h1, h2, h3, h4, h5, h6, blockquote, pre, figure { margin: 0; padding: 0; }

  /* Responsive images */
  img { max-width: 100%; height: auto; }

  /* Links (the email may override) */
  a { color: var(--color-primary, #1a73e8); text-decoration: underline; }
  a:hover { color: #1557b0; }

  /* Tables: avoid the horizontal overflow */
  table { border-collapse: collapse; max-width: 100%; }
  td, th { vertical-align: top; }

  /* Quoted content */
  blockquote {
    border-left: 3px solid #dadce0;
    padding-left: 12px;
    margin: 8px 0 8px 4px;
    color: #5f6368;
  }

  /* Preformatted text */
  pre, code {
    font-family: 'DM Mono', 'Fira Code', monospace;
    font-size: 13px;
    background: #f1f3f4;
    border-radius: 4px;
    padding: 2px 4px;
    white-space: pre-wrap;
    word-break: break-word;
  }
  pre { padding: 12px; }

  hr { border: none; border-top: 1px solid #dadce0; margin: 12px 0; }
`

export default function EmailHtmlView({ html, quoteContext, blockRemoteImages = false, onRemoteImages }: {
  html: string
  quoteContext?: QuoteContext
  /** Hold back images that would be fetched from outside the instance. */
  blockRemoteImages?: boolean
  /** Reports how many remote images the message carries (0 = no banner). */
  onRemoteImages?: (count: number) => void
}) {
  const ref = useRef<HTMLDivElement>(null)
  const ctxKey = `${quoteContext?.to ?? ''}|${quoteContext?.subject ?? ''}|${blockRemoteImages}`

  useEffect(() => {
    const el = ref.current
    if (!el || !html) return

    const shadow = el.shadowRoot ?? el.attachShadow({ mode: 'open' })
    shadow.replaceChildren()

    // 1. Base CSS (interference reset + email defaults).
    const base = document.createElement('style')
    base.textContent = BASE_CSS
    shadow.appendChild(base)

    // 2. The email <style> elements then its FULL <body> (append → the element
    //    survives, so the `body { … }` rules and the inline style / bgcolor apply).
    const { styles, body } = buildEmailNodes(html)
    // Remote images: neutralise (or just count) them while the body is still
    // detached — appending it is what starts the network fetches.
    onRemoteImages?.(blockRemoteImages ? blockRemoteImagesIn(body, styles) : countRemoteImages(body, styles))
    // Fold the quoted history behind a "•••" toggle before it goes on screen, so
    // opening a reply shows the new content, not the whole thread (Gmail-style).
    installQuoteToggle(body, quoteContext)
    styles.forEach(s => shadow.appendChild(s))
    shadow.appendChild(body)

    // Open the links in a new tab (email links must not navigate inside the
    // Kubuno app)
    shadow.querySelectorAll('a[href]').forEach(a => {
      const href = a.getAttribute('href') ?? ''
      if (href.startsWith('http') || href.startsWith('//')) {
        a.setAttribute('target', '_blank')
        a.setAttribute('rel', 'noopener noreferrer')
      }
    })
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [html, ctxKey])

  return <div ref={ref} />
}
