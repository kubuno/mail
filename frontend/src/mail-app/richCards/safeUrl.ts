// Rich-card links are authored by whoever sent the mail (JSON-LD in the body, or
// an attached .ics), so they reach an `href` as fully hostile input. The backend
// now drops unsafe schemes at extraction time, but messages synced BEFORE that
// hardening kept whatever the sender wrote, so the client must re-check every
// link it is about to render rather than trust the stored node.
//
// Only http/https pass: they cannot execute script in the page, whereas
// `javascript:` and `data:text/html` can, and an unknown scheme hands the click
// to an arbitrary local handler. `mailto:` is not allowed either — no card
// renders a mail link (an organizer's address travels as plain text in its own
// field), so allowing it would only widen the surface.
const ALLOWED_URL_SCHEMES = ['http', 'https']

/**
 * The value when it is a link we accept to render, `undefined` otherwise.
 * A URL with no scheme is rejected too: it would resolve against the webmail
 * itself, which is never what a remote sender meant.
 */
export function safeUrl(v: unknown): string | undefined {
  if (typeof v !== 'string') return undefined
  // Browsers ignore ASCII whitespace and C0 controls while parsing the scheme,
  // so `java\tscript:` reaches the same handler as `javascript:`; compare on the
  // same normalized form.
  // eslint-disable-next-line no-control-regex
  const cleaned = v.replace(/[\u0000-\u0020\u007f]/g, '')
  const colon = cleaned.indexOf(':')
  if (colon <= 0) return undefined
  const scheme = cleaned.slice(0, colon).toLowerCase()
  // A colon further down a path (`/a:b`) is not a scheme.
  if (/[/?#]/.test(scheme)) return undefined
  return ALLOWED_URL_SCHEMES.includes(scheme) ? v : undefined
}
