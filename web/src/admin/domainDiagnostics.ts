/**
 * Mail's answer to the core's question "what does this domain's DNS say?".
 *
 * ── Why the core asks instead of reading ─────────────────────────────────────
 * Instance ▸ Domaines does a minimal reading of its own — the MX hosts, whether
 * an SPF record exists, whether a DMARC record exists. It cannot do better: it
 * does not know which MX ought to answer, what an SPF record costs in lookups,
 * which DKIM selector is actually signing, what the outgoing IP's PTR says, or
 * whether the TLS certificate covers the announced name. This module knows all
 * five, and already computes them for its own page.
 *
 * So it registers under the core's `admin.domain-diagnostics` extension point,
 * and the domain sheet renders this instead of its own reading. The core holds
 * no reference to `mail`; this module holds no reference to the core's admin.
 *
 * ⚠️ The contract (key + shapes) is DECLARED IN THE CORE, in
 * `core/frontend/src/core/registry/domainDiagnostics.ts`, and exported by
 * `@kubuno/sdk`. It is mirrored here — same convention as `calendar.view-option`
 * — because a module builds against the PUBLISHED `@kubuno/sdk`, which only
 * carries the contract from its next release. Once published, replace the local
 * types with an import from `@kubuno/sdk` and delete this block. The key string
 * is the real coupling and must never drift.
 */
import { ExtensionRegistry, i18n } from '@kubuno/sdk'
import { mailAdminApi, type DiagnosticCheck } from './api'

// ── Mirror of the core contract (see the header) ─────────────────────────────

const DOMAIN_DIAGNOSTICS = 'admin.domain-diagnostics'

interface DomainDiagnosticCheck {
  id:            string
  kind:          string
  scope:         string
  verdict:       'ok' | 'warn' | 'fail' | 'info' | 'unknown'
  summary:       string
  expected?:     string | null
  found?:        string[]
  instanceWide?: boolean
}

interface DomainDiagnosticReport {
  source:  string
  covered: boolean
  checks:  DomainDiagnosticCheck[]
  note?:   string
  href?:   string
}

interface DomainDiagnosticsProvider {
  fetch: (domain: string) => Promise<DomainDiagnosticReport | null>
}

// ── What belongs to a domain, and what belongs to the instance ───────────────

/** PTR and the certificate are properties of the SERVER, not of one domain: a
 *  missing PTR is not this domain's fault, and hiding it would drop the two
 *  facts that most often explain a rejection. They travel flagged, not merged. */
const INSTANCE_WIDE_KINDS = new Set(['ptr', 'tls'])

/** Who is speaking, in the reader's language — the core prints it verbatim. */
function sourceLabel(): string {
  return i18n.t('mail:diag_source_name', { defaultValue: 'le module Courrier' })
}

/**
 * A check concerns `domain` when its scope IS the domain (mx, spf, dmarc) or
 * sits under it (`<selector>._domainkey.<domain>` for DKIM). Compared in lower
 * case: DNS names are case-insensitive and a selector typed in capitals must
 * not make its key disappear from the sheet.
 */
function concernsDomain(check: DiagnosticCheck, domain: string): boolean {
  const scope = check.scope.toLowerCase().replace(/\.$/, '')
  const name  = domain.toLowerCase().replace(/\.$/, '')
  return scope === name || scope.endsWith(`.${name}`)
}

/** Declares mail as the reader of any domain it serves. Called from `register()`. */
export function registerDomainDiagnostics(): void {
  ExtensionRegistry.register(DOMAIN_DIAGNOSTICS, 'mail', {
    fetch: async (domain: string) => {
      // A failure here is deliberately NOT caught: the core falls back to its
      // own reading, which is the honest outcome — an unreachable module must
      // not leave the operator staring at an empty diagnostic.
      const report = await mailAdminApi.diagnostics()

      const name   = domain.toLowerCase().replace(/\.$/, '')
      const served = report.domains.some(d => d.toLowerCase().replace(/\.$/, '') === name)

      if (!served) {
        // Mail knows this domain is NOT one of its own — which is exactly the
        // link between the two screens the operator is missing. It is said, and
        // the instance's own reading stays below it.
        return {
          source:  sourceLabel(),
          covered: false,
          checks:  [],
          href:    '/admin/modules/mail/addresses',
          note:    i18n.t('mail:diag_domain_not_served', {
            defaultValue:
              'Le module Courrier ne sert pas « {{domain}} » : aucun courrier adressé à ce domaine n’est distribué ici. Les enregistrements ci-dessous sont la lecture de l’instance.',
            domain,
          }),
        } satisfies DomainDiagnosticReport
      }

      const checks = report.checks
        .filter(c => INSTANCE_WIDE_KINDS.has(c.kind) || concernsDomain(c, domain))
        .map<DomainDiagnosticCheck>(c => ({
          id:           c.id,
          kind:         c.kind,
          scope:        c.scope,
          verdict:      c.verdict,
          summary:      c.summary,
          expected:     c.expected,
          found:        c.found,
          instanceWide: INSTANCE_WIDE_KINDS.has(c.kind),
        }))

      return {
        source:  sourceLabel(),
        covered: true,
        checks,
        href:    '/admin/modules/mail/overview',
      } satisfies DomainDiagnosticReport
    },
  } satisfies DomainDiagnosticsProvider)
}
