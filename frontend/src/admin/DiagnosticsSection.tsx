/**
 * Deliverability diagnostic — the first thing on the page, on purpose.
 *
 * ── Why it comes before the settings ─────────────────────────────────────────
 * Mail-in-a-Box built its whole product around "Status Checks", and mailcow and
 * Stalwart put the same page at the top level, for the same reason: a mail
 * server has six external facts (MX, SPF, DKIM, DMARC, PTR, certificate) that
 * decide whether anything is delivered, and none of them lives in the settings
 * screen. Fifty settings with no diagnostic is unreadable; with one, the
 * operator only opens the settings when a light is not green.
 *
 * ── Where this refuses to reassure ───────────────────────────────────────────
 * SPF and DMARC have no single correct value — what to `include:`, whether
 * `p=none` is enough, depends on who else sends for the domain. Those come back
 * as "published, your call": the record is shown, the ten-lookup count is
 * given, and the judgement is the operator's. And a DNS lookup that did not
 * answer is grey, never red — a resolver hiccup must not read as a broken
 * server.
 */
import { useTranslation } from 'react-i18next'
import { useQuery } from '@tanstack/react-query'
import {
  Activity, RefreshCw, Globe, ShieldCheck, FileText, Network, Lock, KeyRound,
} from 'lucide-react'
import { Button, Spinner } from '@ui'
import { mailAdminApi, type DiagnosticCheck } from './api'
import { RecordBlock, Section, VerdictChip, verdictStyle } from './chrome'

const KIND_ICON: Record<string, typeof Globe> = {
  mx:    Network,
  spf:   FileText,
  dkim:  KeyRound,
  dmarc: ShieldCheck,
  ptr:   Globe,
  tls:   Lock,
}

export default function DiagnosticsSection() {
  const { t } = useTranslation('mail')

  const { data, isLoading, isError, isFetching, refetch } = useQuery({
    queryKey: ['mail-admin-diagnostics'],
    queryFn:  mailAdminApi.diagnostics,
    // Nothing here is ours to cache: the operator reads this page right after
    // editing a DNS zone, and a cached answer would say the edit did not take.
    staleTime: 0,
    refetchOnWindowFocus: false,
  })

  return (
    <Section
      icon={<Activity size={16} />}
      title={t('diag_title', { defaultValue: 'Diagnostic de délivrabilité' })}
      description={t('diag_intro', {
        defaultValue:
          'Ce que les fournisseurs destinataires voient de cette instance. Chaque ligne indique ce qui est attendu et ce qui est réellement publié. Un point gris signifie « non vérifiable » (résolveur muet) ou « publié, à votre appréciation » — pas une erreur.',
      })}
      actions={
        <Button variant="secondary" size="sm" onClick={() => void refetch()} loading={isFetching}
          icon={<RefreshCw size={14} />}>
          {t('diag_refresh', { defaultValue: 'Revérifier' })}
        </Button>
      }
    >
      {isLoading ? (
        <div className="flex justify-center py-8"><Spinner /></div>
      ) : isError || !data ? (
        <p className="rounded border border-border bg-surface-1 p-3 text-sm text-text-secondary">
          {t('diag_failed', {
            defaultValue:
              'Le diagnostic n’a pas pu être établi. La configuration du serveur de messagerie est peut-être illisible.',
          })}
        </p>
      ) : (
        <>
          <div className="mb-3 flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-text-secondary">
            <span>
              {t('diag_hostname', { defaultValue: 'Nom annoncé' })} :{' '}
              <span className="font-mono text-text-primary">{data.hostname}</span>
            </span>
            <span>
              {t('diag_domains', { defaultValue: 'Domaines locaux' })} :{' '}
              <span className="font-mono text-text-primary">
                {data.domains.length > 0
                  ? data.domains.join(', ')
                  : t('diag_no_domain', { defaultValue: 'aucun' })}
              </span>
            </span>
          </div>

          {!data.configured && (
            <p className="mb-3 rounded border border-warning bg-warning-light p-2 text-sm text-text-primary">
              {t('diag_no_domain_hint', {
                defaultValue:
                  'Aucun domaine local n’est déclaré : renseignez « Domaines » dans les réglages ci-dessous, sinon il n’y a rien à vérifier et rien à distribuer.',
              })}
            </p>
          )}

          <ul className="space-y-2">
            {data.checks.map(check => <CheckRow key={check.id} check={check} />)}
          </ul>
        </>
      )}
    </Section>
  )
}

function CheckRow({ check }: { check: DiagnosticCheck }) {
  const { t } = useTranslation('mail')
  const Icon  = KIND_ICON[check.kind] ?? Globe
  const style = verdictStyle(check.verdict)

  return (
    <li className="rounded-lg border border-border p-3">
      <div className="mb-1 flex flex-wrap items-center gap-2">
        <Icon size={14} className={style.text} />
        <span className="text-sm font-bold text-text-primary">{check.kind.toUpperCase()}</span>
        <span className="font-mono text-xs text-text-secondary">{check.scope}</span>
        <VerdictChip verdict={check.verdict} />
      </div>

      <p className="mb-2 text-sm leading-relaxed text-text-secondary">{check.summary}</p>

      {check.expected && (
        <div className="mb-2">
          <div className="mb-1 text-xs text-text-tertiary">
            {t('diag_expected', { defaultValue: 'Attendu' })}
          </div>
          <RecordBlock value={check.expected} />
        </div>
      )}

      {check.found.length > 0 && (
        <div>
          <div className="mb-1 text-xs text-text-tertiary">
            {t('diag_found', { defaultValue: 'Publié / constaté' })}
          </div>
          <div className="space-y-1">
            {check.found.map((line, i) => <RecordBlock key={i} value={line} />)}
          </div>
        </div>
      )}
    </li>
  )
}
