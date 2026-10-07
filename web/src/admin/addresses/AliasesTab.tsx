/**
 * Alias — an address that is not a place but a redirection.
 *
 * ── The catch-all is the one dangerous object on this page ───────────────────
 * `@domaine` matches everything in a domain that nothing else matched. Before
 * it exists, an address nobody declared is resolved by the OLD mechanism — a
 * mailbox login, then an account's own address — and reaches whoever that
 * resolves to. The moment a catch-all is created, all of that lands in the
 * catch-all's destinations instead. It is the only operation in this whole
 * panel that redirects mail that was already being delivered, so the form says
 * it in an explicit warning before the button is reachable, not in a tooltip.
 *
 * ── Remote destinations ──────────────────────────────────────────────────────
 * A destination outside every local domain is a FORWARD, and a forward is an
 * outgoing message: it only leaves the instance if outbound delivery is on. The
 * server marks those (`remote_destinations`) and the table shows them apart, so
 * nobody reads a configured forward as a working one.
 */
import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useMutation, useQuery, useQueryClient, keepPreviousData } from '@tanstack/react-query'
import { ArrowLeft, Forward, Globe, Plus, Search, Trash2 } from 'lucide-react'
import {
  Badge, Button, Callout, Checkbox, ConfirmDialog, DataTable, Dropdown, EmptyState,
  FloatingWindow, Input, Textarea, Toggle,
  type DataTableColumn,
} from '@ui'
import { useConfirm } from '@kubuno/sdk'
import { ADDR_KEYS, addressesApi, serverMessage, type Alias } from './api'
import {
  ActiveBadge, AddressField, DomainBadge, ErrorNote, Field,
  addressesToLines, joinAddress, linesToAddresses, splitAddress, useDebounced,
} from './shared'

const PAGE_SIZE = 25

/** Said the same way in the creation form and on an existing catch-all. */
function CatchAllWarning() {
  const { t } = useTranslation('mail')
  return (
    <Callout
      variant="warning"
      className="mb-3"
      title={t('addr_catchall_warn_title', { defaultValue: 'Un attrape-tout détourne du courrier existant' })}
    >
      {t('addr_catchall_warn_body', {
        defaultValue:
          '« @domaine » capture TOUT ce qui n’est ni une boîte, ni un alias, ni une liste — y compris les adresses qui, jusqu’ici, arrivaient par l’ancien mécanisme de résolution (identifiant de boîte, adresse d’un compte). Ce courrier-là changera de destination dès l’enregistrement. C’est le seul réglage de cette page qui redirige du courrier déjà distribué : déclarez d’abord les adresses qui doivent garder leur destinataire.',
      })}
    </Callout>
  )
}

export default function AliasesTab({ servedDomains }: { servedDomains: string[] }) {
  const { t } = useTranslation('mail')
  // `@ui` primitives carry their own strings under `ui.*` in the CORE
  // catalogue: they need the default-namespace translator, not mail's.
  const { t: tui } = useTranslation()
  const qc = useQueryClient()
  const { confirm, confirmState, handleConfirm, handleCancel } = useConfirm()

  const [search,   setSearch]   = useState('')
  const [domain,   setDomain]   = useState('')
  const [page,     setPage]     = useState(0)
  const [selected, setSelected] = useState<string | null>(null)
  const [creating, setCreating] = useState(false)
  const [error,    setError]    = useState<string | null>(null)

  const q = useDebounced(search)
  const query = useMemo(() => ({
    q,
    domain: domain || undefined,
    limit:  PAGE_SIZE,
    offset: page * PAGE_SIZE,
  }), [q, domain, page])

  const list = useQuery({
    queryKey: [ADDR_KEYS.aliases, query],
    queryFn:  () => addressesApi.listAliases(query),
    placeholderData: keepPreviousData,
  })

  const invalidate = () => {
    void qc.invalidateQueries({ queryKey: [ADDR_KEYS.aliases] })
    void qc.invalidateQueries({ queryKey: [ADDR_KEYS.domains] })
  }

  const rows   = list.data?.items ?? []
  const total  = list.data?.total ?? 0
  const record = rows.find(a => a.id === selected) ?? null

  const deleteMut = useMutation({
    mutationFn: (id: string) => addressesApi.deleteAlias(id),
    onSuccess: (result) => {
      setSelected(null)
      setError(null)
      invalidate()
      void confirm({
        title:        t('addr_al_deleted', { defaultValue: 'Alias supprimé' }),
        message:      result.message,
        hideCancel:   true,
        confirmLabel: t('close', { defaultValue: 'Fermer' }),
      })
    },
    onError: (e) => setError(serverMessage(e, t('addr_al_delete_failed', {
      defaultValue: 'Suppression impossible',
    }))),
  })

  const askDelete = async (alias: Alias) => {
    const ok = await confirm({
      title:   t('addr_al_delete_title', { defaultValue: 'Supprimer cet alias ?' }),
      message: alias.is_catch_all
        ? t('addr_al_delete_catchall_msg', {
          defaultValue:
            'L’attrape-tout « {{address}} » ne captera plus rien : les adresses non déclarées du domaine repasseront par l’ancienne résolution, ou seront refusées. Les messages déjà redirigés restent dans les boîtes de destination.',
          address: alias.address,
        })
        : t('addr_al_delete_msg', {
          defaultValue:
            'L’alias « {{address}} » ne sera plus distribué : le courrier qui y arrive sera refusé. Les messages déjà redirigés restent dans les boîtes de destination — supprimer un alias ne supprime aucun message.',
          address: alias.address,
        }),
      variant:      'danger',
      confirmLabel: t('delete', { defaultValue: 'Supprimer' }),
    })
    if (ok) deleteMut.mutate(alias.id)
  }

  const columns: DataTableColumn<Alias>[] = [
    {
      id: 'address',
      header: t('addr_address', { defaultValue: 'Adresse' }),
      headerText: t('addr_address', { defaultValue: 'Adresse' }),
      primary: true,
      required: true,
      minWidth: 220,
      sortValue: r => r.address,
      cell: r => (
        <div className="flex min-w-0 flex-wrap items-center gap-1.5">
          <span className="truncate text-text-primary">{r.address}</span>
          {r.is_catch_all && (
            <Badge variant="warning" size="sm">
              {t('addr_catchall', { defaultValue: 'Attrape-tout' })}
            </Badge>
          )}
          <DomainBadge served={r.domain_served} />
        </div>
      ),
    },
    {
      id: 'destinations',
      header: t('addr_destinations', { defaultValue: 'Destinations' }),
      headerText: t('addr_destinations', { defaultValue: 'Destinations' }),
      minWidth: 260,
      cell: r => (
        <div className="flex min-w-0 flex-wrap gap-1">
          {r.destinations.map(d => {
            const remote = r.remote_destinations.includes(d)
            return (
              <span key={d}
                className={`inline-flex items-center gap-1 rounded px-1.5 py-0.5 ${
                  remote ? 'bg-warning-light text-warning' : 'bg-surface-2 text-text-secondary'}`}
                style={{ fontSize: 'var(--kb-text-micro, 11px)' }}
                title={remote
                  ? t('addr_remote_dest_tip', {
                    defaultValue: 'Destination distante : renvoyée hors de l’instance, seulement si l’envoi sortant est actif.',
                  })
                  : undefined}
              >
                {remote && <Globe size={10} />}
                {d}
              </span>
            )
          })}
        </div>
      ),
    },
    {
      id: 'state',
      header: t('addr_state', { defaultValue: 'État' }),
      headerText: t('addr_state', { defaultValue: 'État' }),
      width: 110,
      sortValue: r => r.is_active,
      cell: r => <ActiveBadge active={r.is_active} />,
    },
    {
      id: 'comment',
      header: t('addr_comment', { defaultValue: 'Commentaire' }),
      headerText: t('addr_comment', { defaultValue: 'Commentaire' }),
      defaultHidden: true,
      minWidth: 160,
      cell: r => <span className="text-text-tertiary">{r.comment ?? '—'}</span>,
    },
  ]

  if (record) {
    return (
      <>
        <AliasRecord
          key={record.id}
          alias={record}
          domains={servedDomains}
          onBack={() => setSelected(null)}
          onDelete={() => void askDelete(record)}
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

      <DataTable<Alias>
        rows={rows}
        columns={columns}
        rowKey={r => r.id}
        loading={list.isLoading}
        error={list.isError
          ? t('addr_al_load_error', { defaultValue: 'La liste des alias n’a pas pu être chargée.' })
          : undefined}
        onRetry={() => void list.refetch()}
        filtered={q.trim().length > 0 || !!domain}
        onClearFilters={() => { setSearch(''); setDomain(''); setPage(0) }}
        manualPagination
        totalRows={total}
        pageSize={PAGE_SIZE}
        page={page}
        onPageChange={setPage}
        configurableColumns
        onRowClick={r => setSelected(r.id)}
        rowActions={[
          {
            id: 'open',
            label: t('addr_open', { defaultValue: 'Ouvrir la fiche' }),
            onClick: r => setSelected(r.id),
          },
          {
            id: 'delete',
            label: t('delete', { defaultValue: 'Supprimer' }),
            icon: <Trash2 size={14} />,
            danger: true,
            onClick: r => void askDelete(r),
          },
        ]}
        emptyState={
          <EmptyState
            variant="first-use"
            icon={<Forward size={26} />}
            title={t('addr_al_empty_title', { defaultValue: 'Aucun alias' })}
            description={t('addr_al_empty_desc', {
              defaultValue:
                'Un alias fait suivre une adresse vers une ou plusieurs autres, locales ou distantes. Utile pour « contact@ », « facturation@ », ou pour rediriger l’adresse d’un collaborateur parti.',
            })}
            action={{
              label: t('addr_al_new', { defaultValue: 'Nouvel alias' }),
              onClick: () => setCreating(true),
            }}
            t={tui}
          />
        }
        toolbar={
          <div className="flex flex-wrap items-center gap-2">
            <div className="relative min-w-[200px]">
              <Search size={14} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-text-tertiary" />
              <Input
                value={search}
                onChange={e => { setSearch(e.target.value); setPage(0) }}
                placeholder={t('addr_al_search', { defaultValue: 'Adresse, destination, commentaire…' })}
                className="pl-8"
                aria-label={t('addr_al_search', { defaultValue: 'Adresse, destination, commentaire…' })}
              />
            </div>
            <Dropdown
              value={domain}
              onChange={v => { setDomain(v); setPage(0) }}
              options={[
                { value: '', label: t('addr_all_domains', { defaultValue: 'Tous les domaines' }) },
                ...servedDomains.map(d => ({ value: d, label: d })),
              ]}
              height={36}
              focusable
            />
            <Button size="sm" icon={<Plus size={14} />} onClick={() => setCreating(true)}>
              {t('addr_al_new', { defaultValue: 'Nouvel alias' })}
            </Button>
          </div>
        }
        t={tui}
      />

      {creating && (
        <CreateAliasWindow
          domains={servedDomains}
          onClose={() => setCreating(false)}
          onCreated={() => { setCreating(false); setError(null); invalidate() }}
        />
      )}

      {confirmState && (
        <ConfirmDialog {...confirmState} onConfirm={handleConfirm} onCancel={handleCancel} />
      )}
    </>
  )
}

// ── Creation ─────────────────────────────────────────────────────────────────

function CreateAliasWindow({ domains, onClose, onCreated }: {
  domains:   string[]
  onClose:   () => void
  onCreated: () => void
}) {
  const { t } = useTranslation('mail')

  const [catchAll, setCatchAll] = useState(false)
  const [local,    setLocal]    = useState('')
  const [domain,   setDomain]   = useState(domains[0] ?? '')
  const [dest,     setDest]     = useState('')
  const [active,   setActive]   = useState(true)
  const [comment,  setComment]  = useState('')
  const [error,    setError]    = useState<string | null>(null)

  const createMut = useMutation({
    mutationFn: () => addressesApi.createAlias({
      address:      catchAll ? `@${domain}` : joinAddress(local, domain),
      destinations: linesToAddresses(dest),
      is_active:    active,
      comment:      comment.trim() || undefined,
    }),
    onSuccess: onCreated,
    onError: (e) => setError(serverMessage(e, t('addr_al_create_failed', {
      defaultValue: 'Création impossible',
    }))),
  })

  const ready = !!domain
    && (catchAll || local.trim().length > 0)
    && linesToAddresses(dest).length > 0

  return (
    <FloatingWindow
      title={t('addr_al_new', { defaultValue: 'Nouvel alias' })}
      icon={<Forward size={16} />}
      onClose={onClose}
      defaultWidth={620}
      backdrop
      padding={20}
    >
      <div className="flex flex-col gap-4">
        <ErrorNote message={error} onDismiss={() => setError(null)} />

        <Checkbox
          checked={catchAll}
          onChange={setCatchAll}
          label={t('addr_make_catchall', { defaultValue: 'Attrape-tout du domaine (@domaine)' })}
          description={t('addr_make_catchall_desc', {
            defaultValue: 'Reçoit tout ce qui n’est ni une boîte, ni un alias, ni une liste. Un seul par domaine.',
          })}
        />

        {/* The one warning that has to be read before the button, not after. */}
        {catchAll && <CatchAllWarning />}

        <Field label={t('addr_address', { defaultValue: 'Adresse' })} htmlFor="al-local">
          {catchAll ? (
            <div className="w-64">
              <Dropdown
                value={domain}
                onChange={setDomain}
                options={domains.map(d => ({ value: d, label: `@${d}` }))}
                height={36}
                width="100%"
                focusable
              />
            </div>
          ) : (
            <AddressField
              id="al-local"
              local={local} domain={domain} domains={domains}
              onLocal={setLocal} onDomain={setDomain}
            />
          )}
        </Field>

        <Field
          label={t('addr_destinations', { defaultValue: 'Destinations' })}
          htmlFor="al-dest"
          hint={t('addr_dest_hint', {
            defaultValue:
              'Une adresse par ligne. Une destination hors des domaines de cette instance est un renvoi vers l’extérieur : il ne part que si l’envoi sortant est actif.',
          })}
        >
          <Textarea
            id="al-dest"
            rows={4}
            value={dest}
            onChange={e => setDest(e.target.value)}
            placeholder={'alice@exemple.fr\nbob@exemple.fr'}
          />
        </Field>

        <Field label={t('addr_comment', { defaultValue: 'Commentaire' })} htmlFor="al-comment">
          <Textarea id="al-comment" rows={2} value={comment}
            onChange={e => setComment(e.target.value)} />
        </Field>

        <Toggle
          checked={active}
          onChange={e => setActive(e.target.checked)}
          label={t('addr_al_active', { defaultValue: 'Alias actif' })}
          description={t('addr_al_active_desc', {
            defaultValue: 'Inactif, l’alias est conservé mais ne distribue rien.',
          })}
        />

        <div className="flex justify-end gap-2">
          <Button variant="ghost" onClick={onClose}>
            {t('cancel', { defaultValue: 'Annuler' })}
          </Button>
          <Button onClick={() => createMut.mutate()} disabled={!ready || createMut.isPending}
            loading={createMut.isPending}>
            {t('create', { defaultValue: 'Créer' })}
          </Button>
        </div>
      </div>
    </FloatingWindow>
  )
}

// ── The record, edited in place ──────────────────────────────────────────────

function AliasRecord({ alias, domains, onBack, onDelete }: {
  alias:    Alias
  domains:  string[]
  onBack:   () => void
  onDelete: () => void
}) {
  const { t } = useTranslation('mail')
  const qc = useQueryClient()

  const split = splitAddress(alias.address)
  const [local,   setLocal]   = useState(split.local)
  const [domain,  setDomain]  = useState(split.domain)
  const [dest,    setDest]    = useState(addressesToLines(alias.destinations))
  const [active,  setActive]  = useState(alias.is_active)
  const [comment, setComment] = useState(alias.comment ?? '')
  const [error,   setError]   = useState<string | null>(null)
  const [saved,   setSaved]   = useState(false)

  const saveMut = useMutation({
    mutationFn: () => addressesApi.updateAlias(alias.id, {
      address:      alias.is_catch_all ? `@${domain}` : joinAddress(local, domain),
      destinations: linesToAddresses(dest),
      is_active:    active,
      comment:      comment.trim(),
    }),
    onSuccess: async (updated) => {
      setError(null)
      setSaved(true)
      window.setTimeout(() => setSaved(false), 2500)
      // Re-seeded from the server BEFORE the refetch: emptying the fields first
      // would paint the stale cached alias back for a frame.
      const next = splitAddress(updated.address)
      setLocal(next.local); setDomain(next.domain)
      setDest(addressesToLines(updated.destinations))
      setActive(updated.is_active)
      setComment(updated.comment ?? '')
      await qc.invalidateQueries({ queryKey: [ADDR_KEYS.aliases] })
      void qc.invalidateQueries({ queryKey: [ADDR_KEYS.domains] })
    },
    onError: (e) => setError(serverMessage(e, t('addr_al_save_failed', {
      defaultValue: 'Enregistrement impossible',
    }))),
  })

  const dirty =
    (alias.is_catch_all ? `@${domain}` : joinAddress(local, domain)) !== alias.address ||
    addressesToLines(linesToAddresses(dest)) !== addressesToLines(alias.destinations) ||
    active !== alias.is_active ||
    comment.trim() !== (alias.comment ?? '')

  return (
    <div className="min-w-0">
      <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
        <Button variant="ghost" size="sm" icon={<ArrowLeft size={14} />} onClick={onBack}>
          {t('addr_al_back', { defaultValue: 'Tous les alias' })}
        </Button>
        <Button variant="ghost" size="sm" icon={<Trash2 size={14} />} onClick={onDelete}>
          {t('delete', { defaultValue: 'Supprimer' })}
        </Button>
      </div>

      <ErrorNote message={error} onDismiss={() => setError(null)} />
      {alias.is_catch_all && <CatchAllWarning />}
      {alias.domain_served === false && (
        <Callout variant="danger" className="mb-3"
          title={t('addr_unserved_title', { defaultValue: 'Ce domaine n’est plus servi' })}>
          {t('addr_al_unserved_body', {
            defaultValue:
              'Le domaine « {{domain}} » ne figure plus dans « server_domains » : cet alias ne reçoit rien.',
            domain: alias.domain,
          })}
        </Callout>
      )}
      {alias.remote_destinations.length > 0 && (
        <Callout variant="info" className="mb-3"
          title={t('addr_remote_title', { defaultValue: 'Renvoi vers l’extérieur' })}>
          {t('addr_remote_body', {
            defaultValue:
              '{{list}} — hors des domaines de cette instance. Ce renvoi n’aboutit que si l’envoi sortant est activé, et le message reste soumis au SPF/DMARC du domaine d’origine.',
            list: alias.remote_destinations.join(', '),
          })}
        </Callout>
      )}

      <div className="grid gap-4 sm:grid-cols-2">
        <Field label={t('addr_address', { defaultValue: 'Adresse' })} htmlFor="alr-local">
          {alias.is_catch_all ? (
            <Dropdown
              value={domain}
              onChange={setDomain}
              options={domains.map(d => ({ value: d, label: `@${d}` }))}
              height={36}
              width="100%"
              focusable
            />
          ) : (
            <AddressField
              id="alr-local"
              local={local} domain={domain} domains={domains}
              onLocal={setLocal} onDomain={setDomain}
            />
          )}
        </Field>
        <Field label={t('addr_comment', { defaultValue: 'Commentaire' })} htmlFor="alr-comment">
          <Textarea id="alr-comment" rows={2} value={comment}
            onChange={e => setComment(e.target.value)} />
        </Field>
      </div>

      <div className="mt-4">
        <Field
          label={t('addr_destinations', { defaultValue: 'Destinations' })}
          htmlFor="alr-dest"
          hint={t('addr_dest_hint_short', {
            defaultValue: 'Une adresse par ligne. Une liste vide est refusée : pour suspendre, désactivez l’alias.',
          })}
        >
          <Textarea id="alr-dest" rows={5} value={dest}
            onChange={e => setDest(e.target.value)} />
        </Field>
      </div>

      <div className="mt-4">
        <Toggle
          checked={active}
          onChange={e => setActive(e.target.checked)}
          label={t('addr_al_active', { defaultValue: 'Alias actif' })}
          description={t('addr_al_active_desc', {
            defaultValue: 'Inactif, l’alias est conservé mais ne distribue rien.',
          })}
        />
      </div>

      <div className="mt-4 flex items-center justify-end gap-2">
        {saved && <span className="text-xs text-success">{t('saved', { defaultValue: 'Enregistré' })}</span>}
        <Button onClick={() => saveMut.mutate()} disabled={!dirty || saveMut.isPending}
          loading={saveMut.isPending}>
          {t('save', { defaultValue: 'Enregistrer' })}
        </Button>
      </div>
    </div>
  )
}
