/**
 * Domaines — where each served domain comes from, what it holds, and the
 * defaults it applies to new boxes.
 *
 * ⚠️ This screen does NOT decide which domains are served. THE INSTANCE IS THE
 * AUTHORITY: the domains served are the VERIFIED ones of Instance ▸ Domaines,
 * plus this module's stand-in list for names DNS cannot prove (`kubuno.local`
 * on a lab machine). A row here only adds what a domain says about ITSELF — a
 * default quota, a ceiling on mailboxes, a note.
 *
 * ── The two states this page exists to make impossible to miss ───────────────
 *  1. A domain that HOLDS objects but is no longer served. Its mailboxes,
 *     aliases and lists look perfectly healthy in the other three tabs, and
 *     nothing reaches any of them. That is what an operator gets by deleting a
 *     domain in Instance ▸ Domaines, so the banner NAMES that cause and offers
 *     the two ways out: declare it again, or add it to the stand-in list.
 *  2. A domain served ONLY by the stand-in list. It works, and the instance
 *     does not claim the name — legitimate in a lab, a mistake in production.
 *
 * Both are said twice: as a banner above the table naming every affected
 * domain, and as a badge on the row (see `domainOrigin.tsx`).
 */
import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { ArrowLeft, Globe, Plus, Trash2 } from 'lucide-react'
import {
  Badge, Button, Callout, ConfirmDialog, DataTable, EmptyState, FloatingWindow,
  Input, Textarea, NumberInput,
  type DataTableColumn,
} from '@ui'
import { useConfirm } from '@kubuno/sdk'
import { ADDR_KEYS, addressesApi, serverMessage, type DomainView } from './api'
import {
  InstanceDomainsLink, OriginBadge, domainOrigin, hasPendingProof, objectCount,
  useOriginSentence,
} from './domainOrigin'
import { ErrorNote, Field, QuotaField, quotaLabel } from './shared'

export default function DomainsTab() {
  const { t } = useTranslation('mail')
  // `@ui` primitives carry their own strings under `ui.*` in the CORE
  // catalogue: they need the default-namespace translator, not mail's.
  const { t: tui } = useTranslation()
  const qc = useQueryClient()
  const { confirm, confirmState, handleConfirm, handleCancel } = useConfirm()

  const [selected, setSelected] = useState<string | null>(null)
  const [adding,   setAdding]   = useState(false)
  const [error,    setError]    = useState<string | null>(null)

  const domains = useQuery({
    queryKey: [ADDR_KEYS.domains],
    queryFn:  addressesApi.listDomains,
  })

  const rows   = domains.data?.items ?? []
  const record = rows.find(d => d.domain === selected) ?? null

  /**
   * The three groups worth a banner, computed once.
   *  • `dropped`  — holds objects and nothing serves it any more.
   *  • `pending`  — declared in the instance, unproved, therefore not served.
   *  • `extra`    — served by the stand-in list alone; the instance claims nothing.
   */
  const groups = useMemo(() => ({
    dropped: rows.filter(d => domainOrigin(d) === 'dropped' && objectCount(d) > 0),
    pending: rows.filter(d => domainOrigin(d) === 'pending'),
    // Served by the stand-in list WHILE the instance's own declaration waits for
    // its proof. Split out of `extra` because the two need opposite sentences:
    // here the instance DOES claim the name — saying it does not would be false,
    // and it is the reading that makes an operator declare it a second time.
    unproved: rows.filter(d => domainOrigin(d) === 'extra' && hasPendingProof(d)),
    extra:    rows.filter(d => domainOrigin(d) === 'extra' && !hasPendingProof(d)),
  }), [rows])

  const pendingWithObjects = groups.pending.filter(d => objectCount(d) > 0)

  const invalidate = () => {
    void qc.invalidateQueries({ queryKey: [ADDR_KEYS.domains] })
  }

  const deleteMut = useMutation({
    mutationFn: (domain: string) => addressesApi.deleteDomainPolicy(domain),
    onSuccess: (result) => {
      setSelected(null)
      setError(null)
      invalidate()
      void confirm({
        title:        t('addr_dom_policy_removed', { defaultValue: 'Politique supprimée' }),
        message:      result.message,
        hideCancel:   true,
        confirmLabel: t('close', { defaultValue: 'Fermer' }),
      })
    },
    onError: (e) => setError(serverMessage(e, t('addr_dom_delete_failed', {
      defaultValue: 'Suppression impossible',
    }))),
  })

  const askDeletePolicy = async (domain: DomainView) => {
    const ok = await confirm({
      title:   t('addr_dom_delete_title', { defaultValue: 'Supprimer la politique de ce domaine ?' }),
      message: t('addr_dom_delete_msg', {
        defaultValue:
          'Les boîtes, alias et listes de « {{domain}} » sont CONSERVÉS, ainsi que leurs quotas actuels. Seules les valeurs par défaut appliquées aux futures boîtes disparaissent, et le plafond de boîtes cesse de s’appliquer.',
        domain: domain.domain,
      }),
      variant:      'danger',
      confirmLabel: t('delete', { defaultValue: 'Supprimer' }),
    })
    if (ok) deleteMut.mutate(domain.domain)
  }

  const originSentence = useOriginSentence()

  const columns: DataTableColumn<DomainView>[] = [
    {
      id: 'domain',
      header: t('addr_domain', { defaultValue: 'Domaine' }),
      headerText: t('addr_domain', { defaultValue: 'Domaine' }),
      primary: true,
      required: true,
      minWidth: 220,
      sortValue: r => r.domain,
      cell: r => (
        <div className="flex min-w-0 flex-wrap items-center gap-1.5">
          <span className="truncate text-text-primary">{r.domain}</span>
          {r.is_served ? (
            <Badge variant="success" size="sm">{t('addr_served', { defaultValue: 'Servi' })}</Badge>
          ) : (
            <Badge variant="danger" size="sm">{t('addr_not_served', { defaultValue: 'Non servi' })}</Badge>
          )}
          {r.has_catch_all && (
            <Badge variant="warning" size="sm">{t('addr_catchall', { defaultValue: 'Attrape-tout' })}</Badge>
          )}
        </div>
      ),
    },
    {
      // The answer to "why is this domain here?", which the two screens never
      // gave before: the instance, the stand-in list, or nothing at all.
      id: 'origin',
      header: t('addr_origin', { defaultValue: 'Provenance' }),
      headerText: t('addr_origin', { defaultValue: 'Provenance' }),
      minWidth: 240,
      sortValue: r => domainOrigin(r),
      cell: r => {
        const sentence = originSentence(r)
        return (
          <span className="flex min-w-0 flex-col items-start gap-0.5">
            <OriginBadge domain={r} />
            {sentence && (
              <span className="text-text-tertiary" style={{ fontSize: 'var(--kb-text-micro, 11px)' }}>
                {sentence}
              </span>
            )}
          </span>
        )
      },
    },
    {
      id: 'objects',
      header: t('addr_objects', { defaultValue: 'Boîtes · alias · listes' }),
      headerText: t('addr_objects', { defaultValue: 'Boîtes · alias · listes' }),
      width: 190,
      sortValue: r => r.mailbox_count + r.alias_count + r.mailing_list_count,
      cell: r => (
        <span className="text-text-secondary">
          {r.mailbox_count} · {r.alias_count} · {r.mailing_list_count}
        </span>
      ),
    },
    {
      id: 'quota',
      header: t('addr_default_quota', { defaultValue: 'Quota par défaut' }),
      headerText: t('addr_default_quota', { defaultValue: 'Quota par défaut' }),
      align: 'right',
      width: 150,
      sortValue: r => r.default_quota_bytes,
      cell: r => (
        <span className="text-text-secondary">
          {r.has_policy
            ? quotaLabel(r.default_quota_bytes, t('addr_unlimited', { defaultValue: 'Illimité' }))
            : '—'}
        </span>
      ),
    },
    {
      id: 'ceiling',
      header: t('addr_max_mailboxes', { defaultValue: 'Plafond de boîtes' }),
      headerText: t('addr_max_mailboxes', { defaultValue: 'Plafond de boîtes' }),
      align: 'right',
      width: 150,
      sortValue: r => r.max_mailboxes,
      cell: r => (
        <span className="text-text-secondary">
          {r.has_policy && r.max_mailboxes > 0
            ? r.max_mailboxes
            : t('addr_no_ceiling', { defaultValue: 'Aucun' })}
        </span>
      ),
    },
  ]

  if (record) {
    return (
      <>
        <DomainRecord
          key={record.domain}
          domain={record}
          onBack={() => setSelected(null)}
          onDeletePolicy={() => void askDeletePolicy(record)}
        />
        {confirmState && (
          <ConfirmDialog {...confirmState} onConfirm={handleConfirm} onCancel={handleCancel} />
        )}
      </>
    )
  }

  return (
    <>
      <ErrorNote message={error} onDismiss={() => setError(null)} />

      {/* The failure that hides best: rows that look healthy everywhere else.
          The cause is NAMED — this is what deleting a domain in Instance ▸
          Domaines produces — and both ways out are offered. */}
      {groups.dropped.length > 0 && (
        <Callout
          variant="danger"
          className="mb-3"
          title={t('addr_inert_title', { defaultValue: 'Des adresses existent dans un domaine que l’instance ne sert plus' })}
        >
          {t('addr_inert_body_v2', {
            defaultValue:
              '{{list}} — rien n’arrive à leurs boîtes, alias ni listes : l’instance répond « destinataire inconnu » pour tout ce domaine. Cause habituelle : le domaine a été retiré d’Instance ▸ Domaines (ou n’y a jamais été vérifié). Redéclarez-le et vérifiez-le, ajoutez-le à la liste d’appoint du module (Services), ou déplacez ses adresses vers un domaine servi.',
            list: groups.dropped.map(d => `${d.domain} (${objectCount(d)})`).join(', '),
          })}
          <span className="ms-1"><InstanceDomainsLink /></span>
        </Callout>
      )}

      {/* Declared upstream, unproved — so not served. One verification away. */}
      {groups.pending.length > 0 && (
        <Callout
          variant={pendingWithObjects.length > 0 ? 'danger' : 'warning'}
          className="mb-3"
          title={t('addr_pending_title', { defaultValue: 'Déclarés dans l’instance, pas encore vérifiés' })}
        >
          {t('addr_pending_body', {
            defaultValue:
              '{{list}} — le domaine figure dans Instance ▸ Domaines mais sa preuve DNS n’a pas été constatée : tant qu’il n’est pas vérifié, il n’est pas servi et le courrier qui lui est adressé est refusé. Terminez la vérification.',
            list: groups.pending.map(d => d.domain).join(', '),
          })}
          <span className="ms-1"><InstanceDomainsLink /></span>
        </Callout>
      )}

      {/* Claimed by the instance, unproved, and reaching mailboxes anyway —
          through the stand-in list. The declaration upstream is doing nothing,
          and an operator who made it almost certainly believes otherwise. */}
      {groups.unproved.length > 0 && (
        <Callout
          variant="warning"
          className="mb-3"
          title={t('addr_unproved_title', {
            defaultValue: 'Déclarés dans l’instance, servis par la liste d’appoint',
          })}
        >
          {t('addr_unproved_body', {
            defaultValue:
              '{{list}} — le domaine figure bien dans Instance ▸ Domaines, mais sa preuve DNS n’a pas été constatée : ce n’est pas elle qui le fait servir, c’est la liste d’appoint du module. Terminez la vérification et l’instance redevient la seule autorité ; retirez-le de la liste d’appoint sans vérifier, et il cesse d’être servi.',
            list: groups.unproved.map(d => d.domain).join(', '),
          })}
          <span className="ms-1"><InstanceDomainsLink /></span>
        </Callout>
      )}

      {/* Served without the instance claiming the name: fine in a lab, a
          mistake in production — so it is signalled, never blocked. */}
      {groups.extra.length > 0 && (
        <Callout
          variant="warning"
          className="mb-3"
          title={t('addr_extra_title', { defaultValue: 'Servis par la liste d’appoint seule' })}
        >
          {t('addr_extra_body', {
            defaultValue:
              '{{list}} — l’instance ne revendique pas ces noms : ils ne sont servis que parce que la liste d’appoint du module les contient. C’est le cas attendu pour un nom que le DNS ne peut pas prouver (kubuno.local). Pour un domaine public, déclarez-le et vérifiez-le dans Instance ▸ Domaines : l’instance redevient la seule autorité.',
            list: groups.extra.map(d => d.domain).join(', '),
          })}
          <span className="ms-1"><InstanceDomainsLink /></span>
        </Callout>
      )}

      <p className="mb-3 leading-relaxed text-text-tertiary"
        style={{ fontSize: 'var(--kb-text-micro, 11px)' }}>
        {t('addr_dom_scope_v2', {
          defaultValue:
            'L’instance fait autorité : sont servis les domaines VÉRIFIÉS d’Instance ▸ Domaines, plus la liste d’appoint du module (pour les noms que le DNS ne peut pas prouver). Cette page ne modifie ni l’une ni l’autre — elle définit seulement ce qu’un domaine applique par défaut aux boîtes qu’on y crée.',
        })}{' '}
        <InstanceDomainsLink />
      </p>

      <DataTable<DomainView>
        rows={rows}
        columns={columns}
        rowKey={r => r.domain}
        loading={domains.isLoading}
        error={domains.isError
          ? t('addr_dom_load_error', {
            defaultValue: 'Les domaines n’ont pas pu être chargés : les réglages du serveur de messagerie sont illisibles.',
          })
          : undefined}
        onRetry={() => void domains.refetch()}
        pageSize={0}
        onRowClick={r => setSelected(r.domain)}
        rowActions={[
          {
            id: 'open',
            label: t('addr_dom_open', { defaultValue: 'Ouvrir la fiche' }),
            onClick: r => setSelected(r.domain),
          },
          {
            id: 'delete',
            label: t('addr_dom_delete_policy', { defaultValue: 'Supprimer la politique' }),
            icon: <Trash2 size={14} />,
            danger: true,
            hidden: r => !r.has_policy,
            onClick: r => void askDeletePolicy(r),
          },
        ]}
        emptyState={
          <EmptyState
            variant="first-use"
            icon={<Globe size={26} />}
            title={t('addr_dom_empty_title', { defaultValue: 'Aucun domaine' })}
            description={t('addr_dom_empty_desc_v2', {
              defaultValue:
                'Aucun domaine n’est servi par cette instance. Déclarez-en un dans Instance ▸ Domaines et vérifiez-le ; pour un nom que le DNS ne peut pas prouver, ajoutez-le à la liste d’appoint du module (Services). Sans domaine servi, aucune adresse locale ne peut exister.',
            })}
            t={tui}
          />
        }
        toolbar={
          <Button size="sm" icon={<Plus size={14} />} onClick={() => setAdding(true)}>
            {t('addr_dom_prepare', { defaultValue: 'Préparer un domaine' })}
          </Button>
        }
        t={tui}
      />

      {adding && (
        <PrepareDomainWindow
          onClose={() => setAdding(false)}
          onSaved={() => { setAdding(false); setError(null); invalidate() }}
        />
      )}

      {confirmState && (
        <ConfirmDialog {...confirmState} onConfirm={handleConfirm} onCancel={handleCancel} />
      )}
    </>
  )
}

// ── Preparing a domain before its MX points here ─────────────────────────────

/**
 * A policy may be written for a domain that is not served YET — an operator
 * prepares a domain before pointing its MX at us. The server accepts it and
 * says plainly that it stays inert; the form repeats that rather than letting a
 * saved policy read as a working domain.
 */
function PrepareDomainWindow({ onClose, onSaved }: {
  onClose: () => void
  onSaved: () => void
}) {
  const { t } = useTranslation('mail')

  const [domain,  setDomain]  = useState('')
  const [quota,   setQuota]   = useState(0)
  const [ceiling, setCeiling] = useState(0)
  const [comment, setComment] = useState('')
  const [error,   setError]   = useState<string | null>(null)
  const [warning, setWarning] = useState<string | null>(null)

  const saveMut = useMutation({
    mutationFn: () => addressesApi.saveDomainPolicy(domain.trim(), {
      default_quota_bytes: quota,
      max_mailboxes:       ceiling,
      comment:             comment.trim(),
    }),
    onSuccess: (body) => {
      setError(null)
      const note = typeof body.warning === 'string' ? body.warning : null
      if (note) setWarning(note)
      else onSaved()
    },
    onError: (e) => setError(serverMessage(e, t('addr_dom_save_failed', {
      defaultValue: 'Enregistrement impossible',
    }))),
  })

  return (
    <FloatingWindow
      title={t('addr_dom_prepare', { defaultValue: 'Préparer un domaine' })}
      icon={<Globe size={16} />}
      onClose={onClose}
      defaultWidth={560}
      backdrop
      padding={20}
    >
      <div className="flex flex-col gap-4">
        <ErrorNote message={error} onDismiss={() => setError(null)} />

        {warning && (
          <Callout variant="warning"
            title={t('addr_dom_saved_inert', { defaultValue: 'Politique enregistrée, mais inerte' })}
            action={{ label: t('close', { defaultValue: 'Fermer' }), onClick: onSaved }}>
            {warning}
          </Callout>
        )}

        <Field
          label={t('addr_domain', { defaultValue: 'Domaine' })}
          htmlFor="dom-name"
          hint={t('addr_dom_prepare_hint_v2', {
            defaultValue:
              'Une politique peut être écrite avant que le domaine soit servi — pendant une migration MX, par exemple. Tant que l’instance ne l’a pas vérifié (ou que la liste d’appoint ne le contient pas), elle reste sans effet.',
          })}
        >
          <Input id="dom-name" value={domain} onChange={e => setDomain(e.target.value)}
            placeholder="exemple.fr" />
        </Field>

        <div className="grid gap-4 sm:grid-cols-2">
          <Field label={t('addr_default_quota', { defaultValue: 'Quota par défaut' })}>
            <QuotaField bytes={quota} onChange={setQuota} />
          </Field>
          <Field
            label={t('addr_max_mailboxes', { defaultValue: 'Plafond de boîtes' })}
            hint={t('addr_ceiling_hint', { defaultValue: '0 = aucun plafond. Jamais appliqué rétroactivement.' })}
          >
            <div className="w-32">
              <NumberInput value={ceiling} min={0} step={1} onChange={setCeiling} />
            </div>
          </Field>
        </div>

        <Field label={t('addr_comment', { defaultValue: 'Commentaire' })} htmlFor="dom-comment">
          <Textarea id="dom-comment" rows={2} value={comment}
            onChange={e => setComment(e.target.value)} />
        </Field>

        <div className="flex justify-end gap-2">
          <Button variant="ghost" onClick={onClose}>
            {t('cancel', { defaultValue: 'Annuler' })}
          </Button>
          <Button onClick={() => saveMut.mutate()}
            disabled={domain.trim().length === 0 || saveMut.isPending}
            loading={saveMut.isPending}>
            {t('save', { defaultValue: 'Enregistrer' })}
          </Button>
        </div>
      </div>
    </FloatingWindow>
  )
}

// ── The record, edited in place ──────────────────────────────────────────────

function DomainRecord({ domain, onBack, onDeletePolicy }: {
  domain:         DomainView
  onBack:         () => void
  onDeletePolicy: () => void
}) {
  const { t } = useTranslation('mail')
  const qc = useQueryClient()

  const [quota,   setQuota]   = useState(domain.default_quota_bytes)
  const [ceiling, setCeiling] = useState(domain.max_mailboxes)
  const [comment, setComment] = useState(domain.comment ?? '')
  const [error,   setError]   = useState<string | null>(null)
  const [saved,   setSaved]   = useState(false)

  const saveMut = useMutation({
    mutationFn: () => addressesApi.saveDomainPolicy(domain.domain, {
      default_quota_bytes: quota,
      max_mailboxes:       ceiling,
      comment:             comment.trim(),
    }),
    onSuccess: async (body) => {
      setError(null)
      setSaved(true)
      window.setTimeout(() => setSaved(false), 2500)
      // Server truth first, refetch second: clearing the fields before the
      // reload lands would paint the previous policy back for a frame.
      if (typeof body.default_quota_bytes === 'number') setQuota(body.default_quota_bytes)
      if (typeof body.max_mailboxes === 'number') setCeiling(body.max_mailboxes)
      setComment(typeof body.comment === 'string' ? body.comment : '')
      await qc.invalidateQueries({ queryKey: [ADDR_KEYS.domains] })
    },
    onError: (e) => setError(serverMessage(e, t('addr_dom_save_failed', {
      defaultValue: 'Enregistrement impossible',
    }))),
  })

  const objects = objectCount(domain)
  const origin  = domainOrigin(domain)
  const originSentence = useOriginSentence()(domain)

  return (
    <div className="min-w-0">
      <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
        <Button variant="ghost" size="sm" icon={<ArrowLeft size={14} />} onClick={onBack}>
          {t('addr_dom_back', { defaultValue: 'Tous les domaines' })}
        </Button>
        {domain.has_policy && (
          <Button variant="ghost" size="sm" icon={<Trash2 size={14} />} onClick={onDeletePolicy}>
            {t('addr_dom_delete_policy', { defaultValue: 'Supprimer la politique' })}
          </Button>
        )}
      </div>

      <ErrorNote message={error} onDismiss={() => setError(null)} />

      <div className="mb-3 flex flex-wrap items-center gap-2">
        <h3 className="text-sm font-bold text-text-primary">{domain.domain}</h3>
        {domain.is_served
          ? <Badge variant="success" size="sm">{t('addr_served', { defaultValue: 'Servi' })}</Badge>
          : <Badge variant="danger" size="sm">{t('addr_not_served', { defaultValue: 'Non servi' })}</Badge>}
        <OriginBadge domain={domain} />
        <span className="text-text-secondary" style={{ fontSize: 'var(--kb-text-micro, 11px)' }}>
          {t('addr_dom_counts', {
            defaultValue: '{{mb}} boîte(s) · {{al}} alias · {{ml}} liste(s)',
            mb: domain.mailbox_count, al: domain.alias_count, ml: domain.mailing_list_count,
          })}
        </span>
      </div>

      {/* Where this row comes from, on one line, with the way to the authority. */}
      {originSentence && (
        <p className="mb-3 flex flex-wrap items-center gap-x-2 gap-y-1 text-text-tertiary"
          style={{ fontSize: 'var(--kb-text-micro, 11px)' }}>
          <span>{originSentence}</span>
          <InstanceDomainsLink />
        </p>
      )}

      {origin === 'pending' && (
        <Callout variant={objects > 0 ? 'danger' : 'warning'} className="mb-3"
          title={t('addr_dom_pending_title', { defaultValue: 'Déclaré dans l’instance, pas encore vérifié' })}>
          {t('addr_dom_pending_body', {
            defaultValue:
              '« {{domain}} » figure dans Instance ▸ Domaines, mais sa preuve DNS n’a pas été constatée : il n’est donc pas servi, et {{count}} objet(s) configurés ici ne reçoivent rien. Terminez la vérification dans Instance ▸ Domaines.',
            domain: domain.domain, count: objects,
          })}
        </Callout>
      )}

      {origin === 'extra' && (
        <Callout variant="warning" className="mb-3"
          title={t('addr_dom_extra_title', { defaultValue: 'Servi par la liste d’appoint seule' })}>
          {t('addr_dom_extra_body', {
            defaultValue:
              'L’instance ne revendique pas « {{domain}} » : il n’est servi que parce que la liste d’appoint du module le contient. Attendu pour un nom que le DNS ne peut pas prouver ; pour un domaine public, déclarez-le et vérifiez-le dans Instance ▸ Domaines.',
            domain: domain.domain,
          })}
        </Callout>
      )}

      {origin === 'dropped' && (
        <Callout variant="danger" className="mb-3"
          title={t('addr_dom_inert_title', { defaultValue: 'Ce domaine ne reçoit rien' })}>
          {t('addr_dom_inert_body_v2', {
            defaultValue:
              '{{count}} objet(s) sont configurés ici, et rien ne sert « {{domain}} » : tout courrier qui lui est adressé est refusé. C’est ce qui arrive quand le domaine est retiré d’Instance ▸ Domaines. Redéclarez-le et vérifiez-le, ou ajoutez-le à la liste d’appoint du module (Services).',
            count: objects, domain: domain.domain,
          })}
        </Callout>
      )}

      {origin === 'idle' && (
        <Callout variant="warning" className="mb-3"
          title={t('addr_dom_prepared_title', { defaultValue: 'Domaine préparé, pas encore servi' })}>
          {t('addr_dom_prepared_body_v2', {
            defaultValue:
              'Cette politique est enregistrée mais sans effet : ni Instance ▸ Domaines ni la liste d’appoint du module ne déclarent ce domaine.',
          })}
        </Callout>
      )}

      {origin === 'unknown' && !domain.is_served && (
        <Callout variant="danger" className="mb-3"
          title={t('addr_dom_inert_title', { defaultValue: 'Ce domaine ne reçoit rien' })}>
          {t('addr_dom_inert_body', {
            defaultValue:
              '{{count}} objet(s) sont configurés ici, et l’instance ne sert pas « {{domain}} » : tout courrier qui lui est adressé est refusé.',
            count: objects, domain: domain.domain,
          })}
        </Callout>
      )}
      {domain.has_catch_all && (
        <Callout variant="info" className="mb-3"
          title={t('addr_dom_catchall_title', { defaultValue: 'Ce domaine a un attrape-tout' })}>
          {t('addr_dom_catchall_body', {
            defaultValue:
              'Toute adresse de ce domaine qui n’est ni une boîte, ni un alias, ni une liste est captée par l’attrape-tout — y compris les erreurs de frappe et les adresses collectées par les spammeurs. Il se modifie dans l’onglet Alias.',
          })}
        </Callout>
      )}

      <div className="grid gap-4 sm:grid-cols-2">
        <Field
          label={t('addr_default_quota', { defaultValue: 'Quota par défaut' })}
          hint={t('addr_default_quota_hint', {
            defaultValue: 'Appliqué aux boîtes créées ensuite sans quota explicite. Les boîtes existantes ne changent pas.',
          })}
        >
          <QuotaField bytes={quota} onChange={setQuota} />
        </Field>
        <Field
          label={t('addr_max_mailboxes', { defaultValue: 'Plafond de boîtes' })}
          hint={t('addr_ceiling_hint', { defaultValue: '0 = aucun plafond. Jamais appliqué rétroactivement.' })}
        >
          <div className="w-32">
            <NumberInput value={ceiling} min={0} step={1} onChange={setCeiling} />
          </div>
        </Field>
      </div>

      <div className="mt-4">
        <Field label={t('addr_comment', { defaultValue: 'Commentaire' })} htmlFor="domr-comment">
          <Textarea id="domr-comment" rows={2} value={comment}
            onChange={e => setComment(e.target.value)} />
        </Field>
      </div>

      <div className="mt-4 flex items-center justify-end gap-2">
        {saved && <span className="text-xs text-success">{t('saved', { defaultValue: 'Enregistré' })}</span>}
        <Button onClick={() => saveMut.mutate()} disabled={saveMut.isPending} loading={saveMut.isPending}>
          {domain.has_policy
            ? t('save', { defaultValue: 'Enregistrer' })
            : t('addr_dom_create_policy', { defaultValue: 'Définir la politique' })}
        </Button>
      </div>
    </div>
  )
}
