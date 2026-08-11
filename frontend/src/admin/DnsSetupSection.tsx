/**
 * DNS setup — "here is what to publish", above the diagnostic's "here is what
 * is wrong".
 *
 * ── Why generate the records rather than describe them ────────────────────────
 * The diagnostic already shows an "expected" value per line, but scattered, in
 * prose, and never grouped by the zone an operator actually edits. Publishing a
 * mail domain means five records — A, MX, SPF, DKIM, DMARC — and the operator
 * pastes them, one at a time, into whatever registrar they use. So this lists
 * them ready to paste, grouped by served domain, each with a copy button, its
 * live status, and one line saying what it is for.
 *
 * ── Two DKIM forms, on purpose ────────────────────────────────────────────────
 * Some registrars (OVH) take the DKIM public key alone — the base64 after `p=`
 * — and build the record themselves; others want the whole TXT value. Guessing
 * wrong means a key that never verifies, so both are offered with their own copy
 * button and a note saying which is which.
 */
import { useTranslation } from 'react-i18next'
import { useQuery } from '@tanstack/react-query'
import {
  Globe2, RefreshCw, Network, FileText, ShieldCheck, KeyRound, Server, ArrowRightLeft,
} from 'lucide-react'
import { Button, Callout, Spinner } from '@ui'
import { mailAdminApi, type DnsRecord } from './api'
import { CopyButton, RecordBlock, Section, VerdictChip } from './chrome'

const KIND_ICON: Record<string, typeof Globe2> = {
  a:     Server,
  mx:    Network,
  spf:   FileText,
  dkim:  KeyRound,
  dmarc: ShieldCheck,
}

export default function DnsSetupSection() {
  const { t } = useTranslation('mail')

  const { data, isLoading, isError, isFetching, refetch } = useQuery({
    queryKey: ['mail-admin-diagnostics'],
    queryFn:  mailAdminApi.diagnostics,
    // Same key and freshness as the diagnostic below: the two read one report,
    // and an operator re-checking after a zone edit must not see a stale value.
    staleTime: 0,
    refetchOnWindowFocus: false,
  })

  const byDomain = groupByDomain(data?.records ?? [])

  return (
    <Section
      icon={<Globe2 size={16} />}
      title={t('dns_title', { defaultValue: 'Mise en route DNS' })}
      description={t('dns_intro', {
        defaultValue:
          'Les enregistrements à publier dans votre zone DNS pour que ce serveur reçoive et envoie le courrier de vos domaines. Copiez-les tels quels chez votre hébergeur DNS ; l’état à droite reflète ce qui est déjà publié.',
      })}
      actions={
        <Button variant="secondary" size="sm" onClick={() => void refetch()} loading={isFetching}
          icon={<RefreshCw size={14} />}>
          {t('dns_refresh', { defaultValue: 'Revérifier' })}
        </Button>
      }
    >
      {isLoading ? (
        <div className="flex justify-center py-8"><Spinner /></div>
      ) : isError || !data ? (
        <p className="rounded border border-border bg-surface-1 p-3 text-sm text-text-secondary">
          {t('dns_failed', {
            defaultValue:
              'Les enregistrements n’ont pas pu être générés. La configuration du serveur de messagerie est peut-être illisible.',
          })}
        </p>
      ) : byDomain.length === 0 ? (
        <Callout variant="info" title={t('dns_empty_title', { defaultValue: 'Aucun domaine servi' })}>
          {t('dns_empty_body', {
            defaultValue:
              'Déclarez et vérifiez d’abord un domaine dans Instance ▸ Domaines, puis ajoutez-y au moins une adresse dans l’onglet Adresses. Les enregistrements DNS à publier apparaîtront alors ici.',
          })}
        </Callout>
      ) : (
        <>
          {data.advisories.length > 0 && (
            <Callout variant="warning" className="mb-4"
              title={t('dns_warn_title', { defaultValue: 'À savoir avant de publier' })}>
              <ul className="list-disc space-y-1 pl-5">
                {data.advisories.map((line, i) => <li key={i}>{line}</li>)}
              </ul>
            </Callout>
          )}

          <div className="space-y-5">
            {byDomain.map(({ domain, records }) => (
              <DomainGroup key={domain} domain={domain} records={records} />
            ))}
          </div>

          <PtrInstruction hostname={data.hostname} />
        </>
      )}
    </Section>
  )
}

/** One served domain and every record grouped beneath its name. */
function DomainGroup({ domain, records }: { domain: string; records: DnsRecord[] }) {
  return (
    <div className="rounded-lg border border-border">
      <div className="border-b border-border bg-surface-1 px-3 py-2">
        <span className="font-mono text-sm font-bold text-text-primary">{domain}</span>
      </div>
      <ul className="divide-y divide-border">
        {records.map(record => <RecordRow key={`${record.key}:${record.name}`} record={record} />)}
      </ul>
    </div>
  )
}

function RecordRow({ record }: { record: DnsRecord }) {
  const { t }  = useTranslation('mail')
  const Icon   = KIND_ICON[record.key] ?? Globe2

  return (
    <li className="p-3">
      <div className="mb-2 flex flex-wrap items-center gap-2">
        <Icon size={14} className="text-text-secondary" />
        <span className="text-sm font-bold text-text-primary">{record.rtype}</span>
        {record.priority != null && (
          <span className="rounded bg-surface-2 px-1.5 py-0.5 font-mono text-text-secondary"
            style={{ fontSize: 'var(--kb-text-micro, 11px)' }}>
            {t('dns_priority', { defaultValue: 'priorité' })} {record.priority}
          </span>
        )}
        <span className="font-mono text-xs text-text-secondary break-all">{record.name}</span>
        <VerdictChip verdict={record.status} />
      </div>

      <RecordBlock value={record.value} />

      {/* DKIM: the full record above, and the bare key here — some hosts want
          one, some the other, and pasting the wrong shape yields a dead key. */}
      {record.value_alt != null && (
        <div className="mt-2">
          <div className="mb-1 flex items-center gap-2 text-xs text-text-tertiary">
            <span>{t('dns_dkim_key_only', { defaultValue: 'Clé seule (base64)' })}</span>
            <CopyButton value={record.value_alt}
              label={t('dns_copy_key_only', { defaultValue: 'Copier la clé seule' })} />
          </div>
          <div className="rounded border border-border bg-surface-1 p-2">
            <div className="font-mono text-xs text-text-primary whitespace-pre-wrap break-all select-all">
              {record.value_alt}
            </div>
          </div>
          <p className="mt-1 text-xs text-text-tertiary">
            {t('dns_dkim_note', {
              defaultValue:
                'Certains hébergeurs, comme OVH, demandent la clé seule ci-dessus ; d’autres l’enregistrement complet.',
            })}
          </p>
        </div>
      )}

      <p className="mt-2 text-xs leading-relaxed text-text-secondary">{record.help}</p>
    </li>
  )
}

/** PTR is not a zone record — it is set at the IP's host. A row of its own so an
 *  operator does not hunt for it in the zone editor, then give up. */
function PtrInstruction({ hostname }: { hostname: string }) {
  const { t } = useTranslation('mail')
  return (
    <div className="mt-4 rounded-lg border border-border p-3">
      <div className="mb-1 flex items-center gap-2">
        <ArrowRightLeft size={14} className="text-text-secondary" />
        <span className="text-sm font-bold text-text-primary">
          {t('dns_ptr_title', { defaultValue: 'PTR (résolution inverse)' })}
        </span>
      </div>
      <p className="text-xs leading-relaxed text-text-secondary">
        {t('dns_ptr_body', {
          defaultValue:
            'Ce n’est pas un enregistrement de zone : réglez la résolution inverse de l’IP publique de votre serveur pour qu’elle renvoie',
        })}{' '}
        <span className="font-mono text-text-primary">{hostname}</span>
        {t('dns_ptr_body_end', {
          defaultValue:
            ', chez l’hébergeur de l’adresse IP (le fournisseur du serveur ou du VPS), pas chez votre hébergeur DNS.',
        })}
      </p>
    </div>
  )
}

/** Groups records by served domain, keeping the backend's order (primary first,
 *  then the record order A ▸ MX ▸ SPF ▸ DKIM ▸ DMARC within each). */
function groupByDomain(records: DnsRecord[]): { domain: string; records: DnsRecord[] }[] {
  const groups: { domain: string; records: DnsRecord[] }[] = []
  for (const record of records) {
    let group = groups.find(g => g.domain === record.domain)
    if (!group) {
      group = { domain: record.domain, records: [] }
      groups.push(group)
    }
    group.records.push(record)
  }
  return groups
}
