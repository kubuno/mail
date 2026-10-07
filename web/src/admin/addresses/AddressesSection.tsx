/**
 * Adresses — the page an operator opens to answer "who has an address here".
 *
 * ── Four tabs, one object ────────────────────────────────────────────────────
 * A mailbox, an alias and a list are three answers to the same question, and
 * the database enforces that an address is exactly one of them. Splitting them
 * across three pages would make "why is contact@ refused" a three-page hunt, so
 * they are three tabs of one panel, with the domains that hold them as a
 * fourth: it is where the counts add up and where an inert domain shows.
 *
 * ── Why the served domains are read here ─────────────────────────────────────
 * Every creation form needs them (a domain is picked, never typed), and every
 * list decorates its rows with them. Reading them once, in the parent, means
 * the four tabs cannot disagree about which domains are served — and it makes
 * "the settings could not be read" a single, honest failure rather than four
 * empty dropdowns.
 */
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useQuery } from '@tanstack/react-query'
import { AtSign, Forward, Globe, Mailbox, Users } from 'lucide-react'
import { Callout, Card, Spinner, Tabs } from '@ui'
import { ADDR_KEYS, addressesApi, serverMessage } from './api'
import MailboxesTab from './MailboxesTab'
import AliasesTab from './AliasesTab'
import ListsTab from './ListsTab'
import DomainsTab from './DomainsTab'

type TabId = 'mailboxes' | 'aliases' | 'lists' | 'domains'

export default function AddressesSection() {
  const { t } = useTranslation('mail')
  // `@ui` primitives carry their own strings under `ui.*` in the CORE
  // catalogue: they need the default-namespace translator, not mail's.
  const { t: tui } = useTranslation()
  const [tab, setTab] = useState<TabId>('mailboxes')

  // The source of truth for "which domains may hold an address", read from the
  // one route that refuses to answer without it.
  const domains = useQuery({
    queryKey: [ADDR_KEYS.domains],
    queryFn:  addressesApi.listDomains,
  })
  const served = domains.data?.served_domains ?? []

  const tabs = [
    { id: 'mailboxes' as const, label: t('addr_tab_mailboxes', { defaultValue: 'Boîtes' }),  icon: Mailbox },
    { id: 'aliases'   as const, label: t('addr_tab_aliases',   { defaultValue: 'Alias' }),   icon: Forward },
    { id: 'lists'     as const, label: t('addr_tab_lists',     { defaultValue: 'Listes de diffusion' }), icon: Users },
    { id: 'domains'   as const, label: t('addr_tab_domains',   { defaultValue: 'Domaines' }), icon: Globe },
  ]

  return (
    <Card
      flush
      className="mb-4"
      icon={<AtSign size={16} />}
      title={t('addr_title', { defaultValue: 'Adresses de cette instance' })}
      subtitle={t('addr_intro', {
        defaultValue:
          'Les adresses locales que cette instance accepte : une boîte classe le courrier dans un compte, un alias le redirige, une liste le distribue à ses membres. Une adresse ne peut être qu’un seul de ces trois objets.',
      })}
    >
      <div className="px-5 pt-3">
        {/* A failure that must not be four empty dropdowns: without the served
            domains nothing here can be created, and saying so once is clearer
            than letting each form look merely unpopulated. */}
        {domains.isError && (
          <Callout
            variant="danger"
            className="mb-3"
            title={t('addr_settings_unreadable', { defaultValue: 'Réglages du serveur illisibles' })}
            action={{
              label: t('retry', { defaultValue: 'Réessayer' }),
              onClick: () => void domains.refetch(),
            }}
          >
            {serverMessage(domains.error, t('addr_settings_unreadable_body', {
              defaultValue:
                'Les domaines servis n’ont pas pu être lus : aucune adresse ne peut être créée tant que cette lecture échoue.',
            }))}
          </Callout>
        )}
        {!domains.isLoading && !domains.isError && served.length === 0 && (
          <Callout
            variant="warning"
            className="mb-3"
            title={t('addr_no_domain_title', { defaultValue: 'Aucun domaine servi' })}
          >
            {t('addr_no_domain_body', {
              defaultValue:
                'Le réglage « server_domains » est vide : cette instance n’accepte le courrier d’aucun domaine, et aucune adresse locale ne peut y être créée. Renseignez-le dans « Services et ports ».',
            })}
          </Callout>
        )}
      </div>

      <Tabs tabs={tabs} value={tab} onChange={setTab} className="px-5" t={tui} />

      <div className="min-w-0 px-5 pb-4 pt-3">
        {domains.isLoading ? (
          <div className="flex justify-center py-10"><Spinner /></div>
        ) : tab === 'mailboxes' ? (
          <MailboxesTab servedDomains={served} />
        ) : tab === 'aliases' ? (
          <AliasesTab servedDomains={served} />
        ) : tab === 'lists' ? (
          <ListsTab servedDomains={served} />
        ) : (
          <DomainsTab />
        )}
      </div>
    </Card>
  )
}
