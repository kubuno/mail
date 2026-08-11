/**
 * WHERE a served domain comes from — the missing link between this screen and
 * Instance ▸ Domaines.
 *
 * ── The confusion this exists to end ─────────────────────────────────────────
 * The two screens used to be unrelated: a domain removed from Instance ▸
 * Domaines went on appearing here as "Servi", and its mailboxes went on looking
 * healthy while nothing reached them. The instance is the authority — the
 * domains served are the VERIFIED ones it claims, plus a stand-in list this
 * module keeps for names DNS cannot prove (`kubuno.local` on a lab machine).
 * So every row has to say which of the two put it there, and what the instance
 * thinks of the name regardless.
 *
 * ── Reading a row the server has not enriched ────────────────────────────────
 * `source` and `instance_state` are optional: an older mail service does not
 * send them. Then the origin is `unknown` and NOTHING is claimed about
 * provenance — an origin invented from `is_served` alone would be wrong exactly
 * when it matters. The "holds objects but is not served" alarm still fires,
 * because that one needs no provenance to be true.
 */
import type { ReactNode } from 'react'
import { useTranslation } from 'react-i18next'
import { Badge } from '@ui'
import type { DomainView } from './api'

/** Instance ▸ Domaines. A real address — opened in a tab, or middle-clicked. */
export const INSTANCE_DOMAINS_URL = '/admin/domains'

export type DomainOrigin =
  /** Served because the instance claims the name AND proved it by DNS. */
  | 'instance'
  /** Served by this module's stand-in list only — the instance claims nothing. */
  | 'extra'
  /** NOT served: declared in the instance, waiting for its DNS proof. */
  | 'pending'
  /** NOT served, and holding mailboxes/aliases/lists. The silent failure. */
  | 'dropped'
  /** NOT served and holding nothing — a policy written ahead of time. */
  | 'idle'
  /** The server does not report provenance. Nothing is claimed. */
  | 'unknown'

export function objectCount(d: DomainView): number {
  return d.mailbox_count + d.alias_count + d.mailing_list_count
}

export function domainOrigin(d: DomainView): DomainOrigin {
  if (d.is_served) {
    // `both` is an instance domain that ALSO sits in the stand-in list. The
    // badge says what matters (the instance claims it); the sentence below
    // flags the redundant entry, which is the part that can bite later.
    if (d.source === 'instance' || d.source === 'both') return 'instance'
    if (d.source === 'extra')                           return 'extra'
    return 'unknown'
  }
  if (d.instance_state === 'pending') return 'pending'
  return objectCount(d) > 0 ? 'dropped' : 'idle'
}

/** A domain served by the stand-in list while the instance's own declaration is
 *  still unproved: served today, and one verification away from being ordinary. */
export function hasPendingProof(d: DomainView): boolean {
  return d.instance_state === 'pending'
}

const VARIANT: Record<DomainOrigin, 'primary' | 'warning' | 'danger' | 'neutral' | 'default'> = {
  instance: 'primary',
  extra:    'warning',
  pending:  'warning',
  dropped:  'danger',
  idle:     'neutral',
  unknown:  'default',
}

/** The origin, as a word. Never the only signal: the served/not-served badge
 *  sits beside it, and the callouts above the table repeat what matters. */
export function OriginBadge({ domain, size = 'sm' }: { domain: DomainView; size?: 'sm' | 'md' }) {
  const { t } = useTranslation('mail')
  const origin = domainOrigin(domain)
  if (origin === 'unknown') return null

  const label: Record<Exclude<DomainOrigin, 'unknown'>, string> = {
    instance: t('addr_origin_instance', { defaultValue: 'Instance' }),
    extra:    t('addr_origin_extra',    { defaultValue: 'Appoint' }),
    pending:  t('addr_origin_pending',  { defaultValue: 'À vérifier' }),
    dropped:  t('addr_origin_dropped',  { defaultValue: 'Retiré de l’instance' }),
    idle:     t('addr_origin_idle',     { defaultValue: 'Non déclaré' }),
  }

  return <Badge variant={VARIANT[origin]} size={size}>{label[origin]}</Badge>
}

/** One sentence explaining the badge, for the row and for the record sheet. */
export function useOriginSentence(): (d: DomainView) => string | null {
  const { t } = useTranslation('mail')
  return (d: DomainView) => {
    switch (domainOrigin(d)) {
      case 'instance':
        return d.source === 'both'
          ? t('addr_origin_both_desc', {
              defaultValue:
                'Déclaré et vérifié dans Instance ▸ Domaines. Il figure aussi dans la liste d’appoint, où l’entrée est redondante — et le resterait servi si vous le retiriez de l’instance.',
            })
          : t('addr_origin_instance_desc', {
              defaultValue: 'Déclaré et vérifié dans Instance ▸ Domaines.',
            })
      case 'extra':
        return t('addr_origin_extra_desc', {
          defaultValue: 'Servi par la liste d’appoint du module : l’instance ne revendique pas ce nom.',
        })
      case 'pending':
        return t('addr_origin_pending_desc', {
          defaultValue: 'Déclaré dans l’instance mais pas encore vérifié — donc pas encore servi.',
        })
      case 'dropped':
        return t('addr_origin_dropped_desc', {
          defaultValue: 'Retiré d’Instance ▸ Domaines, et absent de la liste d’appoint.',
        })
      case 'idle':
        return t('addr_origin_idle_desc', {
          defaultValue: 'Aucune déclaration dans l’instance, aucune entrée d’appoint.',
        })
      default:
        return null
    }
  }
}

/** Instance ▸ Domaines, as a real link — middle-clickable, openable in a tab. */
export function InstanceDomainsLink({ children }: { children?: ReactNode }) {
  const { t } = useTranslation('mail')
  return (
    <a
      href={INSTANCE_DOMAINS_URL}
      target="_blank"
      rel="noopener noreferrer"
      className="text-primary underline underline-offset-2"
    >
      {children ?? t('addr_open_instance_domains', { defaultValue: 'Ouvrir Instance ▸ Domaines' })}
    </a>
  )
}
