/**
 * Listes de diffusion — an address that expands to a membership.
 *
 * ── Why the posting policy is the centre of this screen ──────────────────────
 * A list is an alias with a membership plus ONE question an alias never asks:
 * who may post to it. Getting that wrong is not a cosmetic mistake — an
 * "ouverte" policy on an internal list turns the instance into a spam relay
 * with a ready-made recipient list, and the abuse arrives within days of the
 * address being scraped. So the four policies are radio buttons with a sentence
 * each, read at the moment of choosing, rather than a dropdown of four words.
 *
 * `allowed` with an empty allow-list is refused by the server: it reads as a
 * restriction and means "nobody", which is a list that silently bounces
 * everything. The form says so before the request goes out.
 */
import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useMutation, useQuery, useQueryClient, keepPreviousData } from '@tanstack/react-query'
import { ArrowLeft, Plus, Search, Trash2, Users } from 'lucide-react'
import {
  Badge, Button, Callout, ConfirmDialog, DataTable, Dropdown, EmptyState,
  FloatingWindow, Input, Radio, Textarea, Toggle,
  type DataTableColumn,
} from '@ui'
import { useConfirm } from '@kubuno/sdk'
import { ADDR_KEYS, addressesApi, serverMessage, type MailingList, type PostPolicy } from './api'
import {
  ActiveBadge, AddressField, DomainBadge, ErrorNote, Field,
  addressesToLines, joinAddress, linesToAddresses, splitAddress, useDebounced,
} from './shared'

const PAGE_SIZE = 25

const POLICIES: PostPolicy[] = ['internal', 'members', 'allowed', 'anyone']

/** The four policies, each with the consequence of choosing it. */
function usePolicyText() {
  const { t } = useTranslation('mail')
  const label: Record<PostPolicy, string> = {
    internal: t('addr_pol_internal', { defaultValue: 'Interne' }),
    members:  t('addr_pol_members',  { defaultValue: 'Membres seulement' }),
    allowed:  t('addr_pol_allowed',  { defaultValue: 'Expéditeurs autorisés' }),
    anyone:   t('addr_pol_anyone',   { defaultValue: 'Ouverte' }),
  }
  const description: Record<PostPolicy, string> = {
    internal: t('addr_pol_internal_desc', {
      defaultValue: 'Seules les adresses des domaines servis par cette instance peuvent écrire à la liste. C’est le choix par défaut, et le bon pour une liste d’équipe.',
    }),
    members: t('addr_pol_members_desc', {
      defaultValue: 'Seuls les membres de la liste peuvent y écrire. Un membre parti n’écrit plus ; un partenaire externe non plus.',
    }),
    allowed: t('addr_pol_allowed_desc', {
      defaultValue: 'Seules les adresses explicitement autorisées ci-dessous. Pour une liste d’annonces qu’un service unique alimente. Une liste d’autorisés vide est refusée : elle n’autoriserait personne.',
    }),
    anyone: t('addr_pol_anyone_desc', {
      defaultValue: 'N’importe qui sur Internet peut écrire à la liste, sans authentification. Sur une liste interne, c’est un relais à spam prêt à l’emploi dès que l’adresse est collectée : ne le choisissez que pour une adresse publique assumée.',
    }),
  }
  return { label, description }
}

function PolicyBadge({ policy }: { policy: PostPolicy }) {
  const { label } = usePolicyText()
  return (
    <Badge variant={policy === 'anyone' ? 'warning' : 'neutral'} size="sm">{label[policy]}</Badge>
  )
}

/** The four radios plus the allow-list they gate. */
function PolicyPicker({ policy, onPolicy, allowed, onAllowed }: {
  policy:    PostPolicy
  onPolicy:  (p: PostPolicy) => void
  allowed:   string
  onAllowed: (v: string) => void
}) {
  const { t } = useTranslation('mail')
  const { label, description } = usePolicyText()

  return (
    <div>
      <div className="flex flex-col gap-2">
        {POLICIES.map(p => (
          <Radio
            key={p}
            checked={policy === p}
            onChange={() => onPolicy(p)}
            label={label[p]}
            description={description[p]}
          />
        ))}
      </div>

      {policy === 'anyone' && (
        <Callout variant="warning" className="mt-3"
          title={t('addr_pol_anyone_warn_title', { defaultValue: 'Liste ouverte à Internet' })}>
          {t('addr_pol_anyone_warn_body', {
            defaultValue:
              'Chaque message reçu sera redistribué à tous les membres sans vérification de l’expéditeur. Dès que l’adresse circule, la liste sert de multiplicateur au spam et abîme la réputation d’envoi du domaine. Réservez ce choix à une adresse publique volontairement ouverte.',
          })}
        </Callout>
      )}

      {policy === 'allowed' && (
        <div className="mt-3">
          <Field
            label={t('addr_allowed_senders', { defaultValue: 'Expéditeurs autorisés' })}
            htmlFor="ml-allowed"
            hint={t('addr_allowed_hint', {
              defaultValue: 'Une adresse par ligne. Au moins une est requise, sinon personne ne peut écrire à la liste.',
            })}
          >
            <Textarea id="ml-allowed" rows={3} value={allowed}
              onChange={e => onAllowed(e.target.value)}
              placeholder={'direction@exemple.fr'} />
          </Field>
        </div>
      )}
    </div>
  )
}

export default function ListsTab({ servedDomains }: { servedDomains: string[] }) {
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
    queryKey: [ADDR_KEYS.lists, query],
    queryFn:  () => addressesApi.listMailingLists(query),
    placeholderData: keepPreviousData,
  })

  const invalidate = () => {
    void qc.invalidateQueries({ queryKey: [ADDR_KEYS.lists] })
    void qc.invalidateQueries({ queryKey: [ADDR_KEYS.domains] })
  }

  const rows   = list.data?.items ?? []
  const total  = list.data?.total ?? 0
  const record = rows.find(l => l.id === selected) ?? null

  const deleteMut = useMutation({
    mutationFn: (id: string) => addressesApi.deleteMailingList(id),
    onSuccess: (result) => {
      setSelected(null)
      setError(null)
      invalidate()
      void confirm({
        title:        t('addr_ml_deleted', { defaultValue: 'Liste supprimée' }),
        message:      result.message,
        hideCancel:   true,
        confirmLabel: t('close', { defaultValue: 'Fermer' }),
      })
    },
    onError: (e) => setError(serverMessage(e, t('addr_ml_delete_failed', {
      defaultValue: 'Suppression impossible',
    }))),
  })

  const askDelete = async (ml: MailingList) => {
    const ok = await confirm({
      title:   t('addr_ml_delete_title', { defaultValue: 'Supprimer cette liste ?' }),
      message: t('addr_ml_delete_msg', {
        defaultValue:
          'L’adresse « {{address}} » et ses {{count}} membre(s) sont supprimés : le courrier qui y arrive sera refusé. Les messages déjà distribués aux membres restent dans leurs boîtes.',
        address: ml.address,
        count:   ml.member_count,
      }),
      variant:      'danger',
      confirmLabel: t('delete', { defaultValue: 'Supprimer' }),
    })
    if (ok) deleteMut.mutate(ml.id)
  }

  const columns: DataTableColumn<MailingList>[] = [
    {
      id: 'address',
      header: t('addr_address', { defaultValue: 'Adresse' }),
      headerText: t('addr_address', { defaultValue: 'Adresse' }),
      primary: true,
      required: true,
      minWidth: 220,
      sortValue: r => r.address,
      cell: r => (
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-1.5">
            <span className="truncate text-text-primary">{r.address}</span>
            <DomainBadge served={r.domain_served} />
          </div>
          <div className="truncate text-text-tertiary"
            style={{ fontSize: 'var(--kb-text-micro, 11px)' }}>{r.name}</div>
        </div>
      ),
    },
    {
      id: 'members',
      header: t('addr_members', { defaultValue: 'Membres' }),
      headerText: t('addr_members', { defaultValue: 'Membres' }),
      align: 'right',
      width: 100,
      sortValue: r => r.member_count,
      cell: r => <span className="text-text-secondary">{r.member_count}</span>,
    },
    {
      id: 'policy',
      header: t('addr_policy', { defaultValue: 'Qui peut écrire' }),
      headerText: t('addr_policy', { defaultValue: 'Qui peut écrire' }),
      width: 180,
      sortValue: r => r.post_policy,
      cell: r => <PolicyBadge policy={r.post_policy} />,
    },
    {
      id: 'state',
      header: t('addr_state', { defaultValue: 'État' }),
      headerText: t('addr_state', { defaultValue: 'État' }),
      width: 110,
      sortValue: r => r.is_active,
      cell: r => <ActiveBadge active={r.is_active} />,
    },
  ]

  if (record) {
    return (
      <>
        <ListRecord
          key={record.id}
          list={record}
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

      <DataTable<MailingList>
        rows={rows}
        columns={columns}
        rowKey={r => r.id}
        loading={list.isLoading}
        error={list.isError
          ? t('addr_ml_load_error', { defaultValue: 'La liste des listes de diffusion n’a pas pu être chargée.' })
          : undefined}
        onRetry={() => void list.refetch()}
        filtered={q.trim().length > 0 || !!domain}
        onClearFilters={() => { setSearch(''); setDomain(''); setPage(0) }}
        manualPagination
        totalRows={total}
        pageSize={PAGE_SIZE}
        page={page}
        onPageChange={setPage}
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
            icon={<Users size={26} />}
            title={t('addr_ml_empty_title', { defaultValue: 'Aucune liste de diffusion' })}
            description={t('addr_ml_empty_desc', {
              defaultValue:
                'Une liste distribue le courrier reçu à une adresse vers tous ses membres, en contrôlant qui a le droit d’y écrire.',
            })}
            action={{
              label: t('addr_ml_new', { defaultValue: 'Nouvelle liste' }),
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
                placeholder={t('addr_ml_search', { defaultValue: 'Adresse, nom, commentaire…' })}
                className="pl-8"
                aria-label={t('addr_ml_search', { defaultValue: 'Adresse, nom, commentaire…' })}
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
              {t('addr_ml_new', { defaultValue: 'Nouvelle liste' })}
            </Button>
          </div>
        }
        t={tui}
      />

      {creating && (
        <CreateListWindow
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

function CreateListWindow({ domains, onClose, onCreated }: {
  domains:   string[]
  onClose:   () => void
  onCreated: () => void
}) {
  const { t } = useTranslation('mail')

  const [local,   setLocal]   = useState('')
  const [domain,  setDomain]  = useState(domains[0] ?? '')
  const [name,    setName]    = useState('')
  const [policy,  setPolicy]  = useState<PostPolicy>('internal')
  const [allowed, setAllowed] = useState('')
  const [members, setMembers] = useState('')
  const [comment, setComment] = useState('')
  const [error,   setError]   = useState<string | null>(null)

  const createMut = useMutation({
    mutationFn: () => addressesApi.createMailingList({
      address:         joinAddress(local, domain),
      name:            name.trim(),
      post_policy:     policy,
      allowed_senders: linesToAddresses(allowed),
      members:         linesToAddresses(members),
      comment:         comment.trim() || undefined,
    }),
    onSuccess: onCreated,
    onError: (e) => setError(serverMessage(e, t('addr_ml_create_failed', {
      defaultValue: 'Création impossible',
    }))),
  })

  const ready = local.trim().length > 0 && !!domain && name.trim().length > 0
    && (policy !== 'allowed' || linesToAddresses(allowed).length > 0)

  return (
    <FloatingWindow
      title={t('addr_ml_new', { defaultValue: 'Nouvelle liste' })}
      icon={<Users size={16} />}
      onClose={onClose}
      defaultWidth={660}
      backdrop
      padding={20}
    >
      <div className="flex flex-col gap-4">
        <ErrorNote message={error} onDismiss={() => setError(null)} />

        <div className="grid gap-4 sm:grid-cols-2">
          <Field label={t('addr_address', { defaultValue: 'Adresse' })} htmlFor="ml-local">
            <AddressField
              id="ml-local"
              local={local} domain={domain} domains={domains}
              onLocal={setLocal} onDomain={setDomain}
            />
          </Field>
          <Field label={t('addr_ml_name', { defaultValue: 'Nom de la liste' })} htmlFor="ml-name">
            <Input id="ml-name" value={name} onChange={e => setName(e.target.value)}
              placeholder={t('addr_ml_name_ph', { defaultValue: 'Équipe support' })} />
          </Field>
        </div>

        <Field
          label={t('addr_members', { defaultValue: 'Membres' })}
          htmlFor="ml-members"
          hint={t('addr_members_hint', {
            defaultValue: 'Une adresse par ligne. Une liste sans membre est légitime : elle n’est pas encore peuplée.',
          })}
        >
          <Textarea id="ml-members" rows={4} value={members}
            onChange={e => setMembers(e.target.value)}
            placeholder={'alice@exemple.fr\nbob@exemple.fr'} />
        </Field>

        <Field label={t('addr_policy', { defaultValue: 'Qui peut écrire' })}>
          <PolicyPicker policy={policy} onPolicy={setPolicy}
            allowed={allowed} onAllowed={setAllowed} />
        </Field>

        <Field label={t('addr_comment', { defaultValue: 'Commentaire' })} htmlFor="ml-comment">
          <Textarea id="ml-comment" rows={2} value={comment}
            onChange={e => setComment(e.target.value)} />
        </Field>

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

function ListRecord({ list, domains, onBack, onDelete }: {
  list:     MailingList
  domains:  string[]
  onBack:   () => void
  onDelete: () => void
}) {
  const { t } = useTranslation('mail')
  const qc = useQueryClient()

  const split = splitAddress(list.address)
  const [local,   setLocal]   = useState(split.local)
  const [domain,  setDomain]  = useState(split.domain)
  const [name,    setName]    = useState(list.name)
  const [policy,  setPolicy]  = useState<PostPolicy>(list.post_policy)
  const [allowed, setAllowed] = useState(addressesToLines(list.allowed_senders))
  const [members, setMembers] = useState(addressesToLines(list.members))
  const [active,  setActive]  = useState(list.is_active)
  const [comment, setComment] = useState(list.comment ?? '')
  const [error,   setError]   = useState<string | null>(null)
  const [saved,   setSaved]   = useState(false)

  const saveMut = useMutation({
    mutationFn: () => addressesApi.updateMailingList(list.id, {
      address:         joinAddress(local, domain),
      name:            name.trim(),
      post_policy:     policy,
      allowed_senders: linesToAddresses(allowed),
      members:         linesToAddresses(members),
      is_active:       active,
      comment:         comment.trim(),
    }),
    onSuccess: async (updated) => {
      setError(null)
      setSaved(true)
      window.setTimeout(() => setSaved(false), 2500)
      // Server truth first, refetch second — see MailboxRecord.
      const next = splitAddress(updated.address)
      setLocal(next.local); setDomain(next.domain)
      setName(updated.name)
      setPolicy(updated.post_policy)
      setAllowed(addressesToLines(updated.allowed_senders))
      setMembers(addressesToLines(updated.members))
      setActive(updated.is_active)
      setComment(updated.comment ?? '')
      await qc.invalidateQueries({ queryKey: [ADDR_KEYS.lists] })
      void qc.invalidateQueries({ queryKey: [ADDR_KEYS.domains] })
    },
    onError: (e) => setError(serverMessage(e, t('addr_ml_save_failed', {
      defaultValue: 'Enregistrement impossible',
    }))),
  })

  const dirty =
    joinAddress(local, domain) !== list.address ||
    name.trim() !== list.name ||
    policy !== list.post_policy ||
    addressesToLines(linesToAddresses(allowed)) !== addressesToLines(list.allowed_senders) ||
    addressesToLines(linesToAddresses(members)) !== addressesToLines(list.members) ||
    active !== list.is_active ||
    comment.trim() !== (list.comment ?? '')

  const blocked = policy === 'allowed' && linesToAddresses(allowed).length === 0

  return (
    <div className="min-w-0">
      <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
        <Button variant="ghost" size="sm" icon={<ArrowLeft size={14} />} onClick={onBack}>
          {t('addr_ml_back', { defaultValue: 'Toutes les listes' })}
        </Button>
        <Button variant="ghost" size="sm" icon={<Trash2 size={14} />} onClick={onDelete}>
          {t('delete', { defaultValue: 'Supprimer' })}
        </Button>
      </div>

      <ErrorNote message={error} onDismiss={() => setError(null)} />
      {list.domain_served === false && (
        <Callout variant="danger" className="mb-3"
          title={t('addr_unserved_title', { defaultValue: 'Ce domaine n’est plus servi' })}>
          {t('addr_ml_unserved_body', {
            defaultValue:
              'Le domaine « {{domain}} » ne figure plus dans « server_domains » : cette liste ne reçoit rien.',
            domain: list.domain,
          })}
        </Callout>
      )}

      <div className="grid gap-4 sm:grid-cols-2">
        <Field label={t('addr_address', { defaultValue: 'Adresse' })} htmlFor="mlr-local">
          <AddressField
            id="mlr-local"
            local={local} domain={domain} domains={domains}
            onLocal={setLocal} onDomain={setDomain}
          />
        </Field>
        <Field label={t('addr_ml_name', { defaultValue: 'Nom de la liste' })} htmlFor="mlr-name">
          <Input id="mlr-name" value={name} onChange={e => setName(e.target.value)} />
        </Field>
      </div>

      <div className="mt-4 grid gap-4 sm:grid-cols-2">
        <Field
          label={t('addr_members', { defaultValue: 'Membres' })}
          htmlFor="mlr-members"
          hint={t('addr_members_count', {
            defaultValue: '{{count}} membre(s) enregistré(s). Une adresse par ligne.',
            count: list.member_count,
          })}
        >
          <Textarea id="mlr-members" rows={7} value={members}
            onChange={e => setMembers(e.target.value)} />
        </Field>
        <Field label={t('addr_policy', { defaultValue: 'Qui peut écrire' })}>
          <PolicyPicker policy={policy} onPolicy={setPolicy}
            allowed={allowed} onAllowed={setAllowed} />
        </Field>
      </div>

      <div className="mt-4 grid gap-4 sm:grid-cols-2">
        <Field label={t('addr_comment', { defaultValue: 'Commentaire' })} htmlFor="mlr-comment">
          <Textarea id="mlr-comment" rows={2} value={comment}
            onChange={e => setComment(e.target.value)} />
        </Field>
        <Field label={t('addr_state', { defaultValue: 'État' })}>
          <Toggle
            checked={active}
            onChange={e => setActive(e.target.checked)}
            label={t('addr_ml_active', { defaultValue: 'Liste active' })}
            description={t('addr_ml_active_desc', {
              defaultValue: 'Inactive, la liste est conservée mais ne distribue rien.',
            })}
          />
        </Field>
      </div>

      <div className="mt-4 flex items-center justify-end gap-2">
        {blocked && (
          <span className="text-xs text-danger">
            {t('addr_allowed_empty', {
              defaultValue: 'Ajoutez au moins un expéditeur autorisé.',
            })}
          </span>
        )}
        {saved && <span className="text-xs text-success">{t('saved', { defaultValue: 'Enregistré' })}</span>}
        <Button onClick={() => saveMut.mutate()} disabled={!dirty || blocked || saveMut.isPending}
          loading={saveMut.isPending}>
          {t('save', { defaultValue: 'Enregistrer' })}
        </Button>
      </div>
    </div>
  )
}
