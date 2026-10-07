import { useQuery } from '@tanstack/react-query'
import { mailApi } from '../api'
import { loadPrefs } from '../settings/prefs'

/** One place decides whether a message's remote images load, so the banner and
 *  the renderer can never disagree. Gmail's rule, transposed:
 *   1. the user asked for images to always display  → show
 *   2. the sender is on the instance allowlist (administrator, applies to every
 *      account, the user cannot remove it)          → show
 *   3. the sender is on the user's own allowlist    → show
 *   4. otherwise                                    → hold them back and ask
 */
export function useImagePolicy() {
  // Cached across messages: the lists change only when the user allows a sender.
  const { data } = useQuery({
    queryKey: ['mail-image-senders'],
    queryFn:  mailApi.listImageSenders,
    staleTime: 60_000,
  })
  const alwaysShow = loadPrefs().showImages === 'always'
  const mine     = (data?.senders ?? []).map(s => s.email.toLowerCase())
  const instance = (data?.instance ?? []).map(s => s.toLowerCase())

  /** Does an entry ("someone@x.com" or "@x.com") cover this address? */
  const covers = (entry: string, email: string) =>
    entry.startsWith('@') ? email.endsWith(entry) : entry === email

  return {
    alwaysShow,
    /** The sender is trusted for images — by the instance or by the user.
     *  The INSTANCE list only applies to an authenticated sender (DMARC pass):
     *  nobody opted into it individually, so a spoofed From must not be able to
     *  ride it. A sender the user trusted themselves is honoured as chosen. */
    isAllowed(fromEmail: string, dmarc?: string | null): boolean {
      const email = fromEmail.trim().toLowerCase()
      if (!email) return false
      if (mine.some(e => covers(e, email))) return true
      return dmarc === 'pass' && instance.some(e => covers(e, email))
    },
    /** True when the sender is trusted instance-wide: the user is told images
     *  are allowed by policy rather than offered a button that changes nothing. */
    isInstanceAllowed(fromEmail: string, dmarc?: string | null): boolean {
      const email = fromEmail.trim().toLowerCase()
      return !!email && dmarc === 'pass' && instance.some(e => covers(e, email))
    },
  }
}
