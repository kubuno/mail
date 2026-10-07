/**
 * Boîtes — the local addresses this instance files into a Kubuno account.
 *
 * ── Three things this screen refuses to do ───────────────────────────────────
 *  • Show an occupancy. `mail.messages` stores no size and no delivered-to
 *    address, so a bar at 0 % would read as "empty" for a mailbox holding ten
 *    years of correspondence. The quota is shown; the absence of a figure is
 *    stated once, above the table (`usage_note`).
 *  • Let a domain be typed. It is picked from the domains the instance serves,
 *    because an address in any other domain is a row that will never receive
 *    anything and nobody finds out for weeks.
 *  • Let the one-shot password vanish. It exists in readable form exactly once,
 *    in the creation response; the panel that shows it is dismissed by an
 *    explicit click and by nothing else.
 *
 * ── Editing happens on the record, creation in a window ──────────────────────
 * Opening a mailbox replaces the list with its record, and every field is
 * edited in place there — an address, an owner and a quota are read together
 * and changed one at a time. Only the CREATION is a modal, because it is the
 * one moment nothing exists yet to be read.
 */
import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useMutation, useQuery, useQueryClient, keepPreviousData } from '@tanstack/react-query'
import {
  ArrowLeft, AtSign, KeyRound, Mailbox as MailboxIcon, Plus, Search, Trash2, UserX,
} from 'lucide-react'
import {
  Badge, Button, Callout, Checkbox, ConfirmDialog, DataTable, Dropdown, EmptyState,
  FloatingWindow, Input, Textarea, Toggle,
  type DataTableColumn,
} from '@ui'
import { useConfirm } from '@kubuno/sdk'
import {
  ADDR_KEYS, addressesApi, serverMessage,
  type CreatedCredential, type DirectoryUser, type Mailbox,
} from './api'
import {
  ActiveBadge, AddressField, CredentialPanel, DomainBadge, ErrorNote, Field, OwnerField,
  QuotaField, UsageNote, joinAddress, quotaLabel, splitAddress, useDebounced,
} from './shared'

const PAGE_SIZE = 25

export default function MailboxesTab({ servedDomains }: { servedDomains: string[] }) {
  const { t } = useTranslation('mail')
  // `@ui` primitives carry their own strings under `ui.*` in the CORE
  // catalogue: they need the default-namespace translator, not mail's.
  const { t: tui } = useTranslation()
  const qc = useQueryClient()
  const { confirm, confirmState, handleConfirm, handleCancel } = useConfirm()

  const [search,   setSearch]   = useState('')
  const [domain,   setDomain]   = useState('')
  const [active,   setActive]   = useState<'' | 'true' | 'false'>('')
  const [page,     setPage]     = useState(0)
  const [selected, setSelected] = useState<string | null>(null)
  const [creating, setCreating] = useState(false)
  const [error,    setError]    = useState<string | null>(null)
  const [credential, setCredential] = useState<CreatedCredential | null>(null)

  const q = useDebounced(search)

  const query = useMemo(() => ({
    q,
    domain: domain || undefined,
    active: active === '' ? undefined : active === 'true',
    limit:  PAGE_SIZE,
    offset: page * PAGE_SIZE,
  }), [q, domain, active, page])

  const list = useQuery({
    queryKey:       [ADDR_KEYS.mailboxes, query],
    queryFn:        () => addressesApi.listMailboxes(query),
    // Keeps the previous page on screen while the next one loads, so the table
    // does not collapse to a skeleton on every keystroke.
    placeholderData: keepPreviousData,
  })

  // The owner picker's source, also used to name the owner in the table: a
  // column showing a bare UUID is a column nobody reads.
  const directory = useQuery({
    queryKey: [ADDR_KEYS.directory],
    queryFn:  () => addressesApi.directory('', 200),
    staleTime: 60_000,
  })
  const users = directory.data ?? []
  const nameOf = useMemo(() => {
    const map = new Map<string, DirectoryUser>()
    for (const u of users) map.set(u.id, u)
    return (id: string) => {
      const u = map.get(id)
      return u ? (u.display_name || u.username || u.email || id) : id
    }
  }, [users])

  const invalidate = () => {
    void qc.invalidateQueries({ queryKey: [ADDR_KEYS.mailboxes] })
    // A new mailbox changes the per-domain counts, and may hit a ceiling there.
    void qc.invalidateQueries({ queryKey: [ADDR_KEYS.domains] })
  }

  const rows    = list.data?.items ?? []
  const total   = list.data?.total ?? 0
  const record  = rows.find(m => m.id === selected) ?? null
  const filtered = q.trim().length > 0 || !!domain || active !== ''

  // ── Delete ─────────────────────────────────────────────────────────────────
  const deleteMut = useMutation({
    mutationFn: ({ id, cred }: { id: string; cred: boolean }) =>
      addressesApi.deleteMailbox(id, cred),
    onSuccess: (result) => {
      setSelected(null)
      setError(null)
      invalidate()
      // The server's own sentence: what was kept, and what was not.
      void confirm({
        title:      t('addr_mb_deleted', { defaultValue: 'Boîte supprimée' }),
        message:    result.message,
        hideCancel: true,
        confirmLabel: t('close', { defaultValue: 'Fermer' }),
      })
    },
    onError: (e) => setError(serverMessage(e, t('addr_mb_delete_failed', {
      defaultValue: 'Suppression impossible',
    }))),
  })

  const askDelete = async (mailbox: Mailbox) => {
    // Said BEFORE, not after: an administrator deleting an address needs to know
    // that the messages already delivered are NOT going away.
    const ok = await confirm({
      title:   t('addr_mb_delete_title', { defaultValue: 'Supprimer cette boîte ?' }),
      message: t('addr_mb_delete_msg', {
        defaultValue:
          '« {{address}} » n’acceptera plus de courrier. Les messages DÉJÀ distribués au compte propriétaire sont conservés : supprimer une boîte ne supprime aucun message.',
        address: mailbox.address,
      }),
      variant: 'danger',
      confirmLabel: t('delete', { defaultValue: 'Supprimer' }),
    })
    if (!ok) return

    // The IMAP/SMTP login is a SEPARATE object that outlives the address unless
    // asked otherwise — and one that may predate this table. Deleting it
    // silently would cut off a mail client that has worked for months; keeping
    // it silently would leave a working login for an address that no longer
    // exists. Neither is a default worth guessing, so it is a second question,
    // asked only when there is actually a login to decide about.
    let cred = false
    if (mailbox.has_credential) {
      cred = await confirm({
        title:   t('addr_mb_delete_cred_title', { defaultValue: 'Supprimer aussi l’identifiant IMAP/SMTP ?' }),
        message: t('addr_mb_delete_cred_msg', {
          defaultValue:
            'Un identifiant porte « {{address}} ». Le supprimer empêche toute connexion IMAP/SMTP avec ce mot de passe ; le conserver laisse un accès actif à un compte dont l’adresse vient d’être supprimée.',
          address: mailbox.address,
        }),
        variant:      'warning',
        confirmLabel: t('addr_mb_delete_cred_yes', { defaultValue: 'Supprimer aussi' }),
        cancelLabel:  t('addr_mb_delete_cred_no',  { defaultValue: 'Le conserver' }),
      })
    }
    deleteMut.mutate({ id: mailbox.id, cred })
  }

  // ── Columns ────────────────────────────────────────────────────────────────
  const columns: DataTableColumn<Mailbox>[] = [
    {
      id: 'address',
      header: t('addr_address', { defaultValue: 'Adresse' }),
      headerText: t('addr_address', { defaultValue: 'Adresse' }),
      primary: true,
      required: true,
      minWidth: 240,
      sortValue: r => r.address,
      cell: r => (
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-1.5">
            <span className="truncate text-text-primary">{r.address}</span>
            <DomainBadge served={r.domain_served} />
          </div>
          {r.display_name && (
            <div className="truncate text-text-tertiary"
              style={{ fontSize: 'var(--kb-text-micro, 11px)' }}>{r.display_name}</div>
          )}
        </div>
      ),
    },
    {
      id: 'owner',
      header: t('addr_owner', { defaultValue: 'Propriétaire' }),
      headerText: t('addr_owner', { defaultValue: 'Propriétaire' }),
      minWidth: 180,
      sortValue: r => r.user_id,
      cell: r => (
        <div className="min-w-0">
          <div className="truncate text-text-secondary">{nameOf(r.user_id)}</div>
          {!r.owner_known && (
            <span className="mt-0.5 inline-flex items-center gap-1">
              <UserX size={11} className="text-danger" />
              <Badge variant="danger" size="sm">
                {t('addr_owner_unknown', { defaultValue: 'Compte inconnu du module' })}
              </Badge>
            </span>
          )}
        </div>
      ),
    },
    {
      id: 'quota',
      header: t('addr_quota', { defaultValue: 'Quota' }),
      headerText: t('addr_quota', { defaultValue: 'Quota' }),
      align: 'right',
      width: 110,
      sortValue: r => r.quota_bytes,
      cell: r => (
        <span className="text-text-secondary">
          {quotaLabel(r.quota_bytes, t('addr_unlimited', { defaultValue: 'Illimité' }))}
        </span>
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
      id: 'credential',
      header: t('addr_cred_col', { defaultValue: 'Identifiant IMAP/SMTP' }),
      headerText: t('addr_cred_col', { defaultValue: 'Identifiant IMAP/SMTP' }),
      width: 170,
      sortValue: r => r.has_credential,
      cell: r => r.has_credential
        ? <Badge variant="neutral" size="sm">{t('addr_cred_yes', { defaultValue: 'Configuré' })}</Badge>
        : <span className="text-text-tertiary">{t('addr_cred_no', { defaultValue: 'Aucun' })}</span>,
    },
    {
      id: 'messages',
      header: t('addr_owner_msgs', { defaultValue: 'Messages du compte' }),
      headerText: t('addr_owner_msgs', { defaultValue: 'Messages du compte' }),
      align: 'right',
      width: 150,
      defaultHidden: true,
      sortValue: r => r.owner_message_count,
      cell: r => <span className="text-text-tertiary">{r.owner_message_count}</span>,
    },
  ]

  // ── The record ─────────────────────────────────────────────────────────────
  if (record) {
    return (
      <>
        {credential && (
          <CredentialPanel {...credential} onDone={() => setCredential(null)} />
        )}
        <MailboxRecord
          key={record.id}
          mailbox={record}
          domains={servedDomains}
          users={users}
          directoryError={directory.isError
            ? t('addr_dir_error', { defaultValue: 'L’annuaire des comptes est injoignable : le propriétaire ne peut pas être changé pour l’instant.' })
            : null}
          directoryLoading={directory.isLoading}
          onBack={() => setSelected(null)}
          onCredential={setCredential}
          onDelete={() => void askDelete(record)}
        />
        {confirmState && (
          <ConfirmDialog {...confirmState} onConfirm={handleConfirm} onCancel={handleCancel} />
        )}
      </>
    )
  }

  // ── The list ───────────────────────────────────────────────────────────────
  return (
    <>
      {credential && (
        <CredentialPanel {...credential} onDone={() => setCredential(null)} />
      )}
      <ErrorNote message={error} onDismiss={() => setError(null)} />
      <UsageNote note={list.data?.usage_note ?? ''} />

      <DataTable<Mailbox>
        rows={rows}
        columns={columns}
        rowKey={r => r.id}
        loading={list.isLoading}
        error={list.isError
          ? t('addr_mb_load_error', { defaultValue: 'La liste des boîtes n’a pas pu être chargée.' })
          : undefined}
        onRetry={() => void list.refetch()}
        filtered={filtered}
        onClearFilters={() => { setSearch(''); setDomain(''); setActive(''); setPage(0) }}
        manualPagination
        manualSort={false}
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
            icon={<MailboxIcon size={26} />}
            title={t('addr_mb_empty_title', { defaultValue: 'Aucune adresse locale' })}
            description={t('addr_mb_empty_desc', {
              defaultValue:
                'Tant qu’aucune boîte n’existe, cette instance refuse le courrier entrant de ses propres domaines (550 destinataire inconnu). Créez la première adresse.',
            })}
            action={{
              label: t('addr_mb_new', { defaultValue: 'Nouvelle adresse' }),
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
                placeholder={t('addr_mb_search', { defaultValue: 'Adresse, nom, commentaire…' })}
                className="pl-8"
                aria-label={t('addr_mb_search', { defaultValue: 'Adresse, nom, commentaire…' })}
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
            <Dropdown
              value={active}
              onChange={v => { setActive(v as '' | 'true' | 'false'); setPage(0) }}
              options={[
                { value: '',      label: t('addr_all_states', { defaultValue: 'Tous les états' }) },
                { value: 'true',  label: t('addr_active', { defaultValue: 'Actif' }) },
                { value: 'false', label: t('addr_suspended', { defaultValue: 'Suspendu' }) },
              ]}
              height={36}
              focusable
            />
            <Button size="sm" icon={<Plus size={14} />} onClick={() => setCreating(true)}>
              {t('addr_mb_new', { defaultValue: 'Nouvelle adresse' })}
            </Button>
          </div>
        }
        t={tui}
      />

      {creating && (
        <CreateMailboxWindow
          domains={servedDomains}
          users={users}
          directoryLoading={directory.isLoading}
          directoryError={directory.isError
            ? t('addr_dir_error_create', {
              defaultValue: 'L’annuaire des comptes est injoignable : impossible de choisir un propriétaire. Réessayez une fois le service revenu.',
            })
            : null}
          onClose={() => setCreating(false)}
          onCreated={(created) => {
            setCreating(false)
            setError(null)
            if (created.credential) setCredential(created.credential)
            invalidate()
          }}
        />
      )}

      {confirmState && (
        <ConfirmDialog {...confirmState} onConfirm={handleConfirm} onCancel={handleCancel} />
      )}
    </>
  )
}

// ── Creation ─────────────────────────────────────────────────────────────────

function CreateMailboxWindow({
  domains, users, directoryLoading, directoryError, onClose, onCreated,
}: {
  domains: string[]
  users:   DirectoryUser[]
  directoryLoading: boolean
  directoryError:   string | null
  onClose:   () => void
  onCreated: (created: { credential: CreatedCredential | null }) => void
}) {
  const { t } = useTranslation('mail')

  const [local,  setLocal]  = useState('')
  const [domain, setDomain] = useState(domains[0] ?? '')
  const [owner,  setOwner]  = useState('')
  const [name,   setName]   = useState('')
  const [quota,  setQuota]  = useState(0)
  const [withCredential, setWithCredential] = useState(true)
  const [replace, setReplace] = useState(false)
  const [comment, setComment] = useState('')
  const [error,   setError]   = useState<string | null>(null)

  const createMut = useMutation({
    mutationFn: () => addressesApi.createMailbox({
      address:      joinAddress(local, domain),
      user_id:      owner,
      display_name: name.trim() || undefined,
      quota_bytes:  quota,
      comment:      comment.trim() || undefined,
      create_credential:  withCredential,
      replace_credential: replace,
    }),
    onSuccess: (created) => onCreated(created),
    onError: (e) => setError(serverMessage(e, t('addr_mb_create_failed', {
      defaultValue: 'Création impossible',
    }))),
  })

  const ready = local.trim().length > 0 && !!domain && !!owner

  return (
    <FloatingWindow
      title={t('addr_mb_new', { defaultValue: 'Nouvelle adresse' })}
      icon={<AtSign size={16} />}
      onClose={onClose}
      defaultWidth={620}
      backdrop
      padding={20}
    >
      <div className="flex flex-col gap-4">
        <ErrorNote message={error} onDismiss={() => setError(null)} />

        <Field
          label={t('addr_address', { defaultValue: 'Adresse' })}
          htmlFor="mb-local"
          hint={t('addr_domain_hint', {
            defaultValue: 'Le domaine est choisi parmi ceux que cette instance sert : une adresse ailleurs ne recevrait jamais rien.',
          })}
        >
          <AddressField
            id="mb-local"
            local={local} domain={domain} domains={domains}
            onLocal={setLocal} onDomain={setDomain}
          />
        </Field>

        <Field
          label={t('addr_owner', { defaultValue: 'Propriétaire' })}
          hint={t('addr_owner_hint', {
            defaultValue: 'Le compte Kubuno dans lequel le courrier reçu à cette adresse sera classé.',
          })}
        >
          <OwnerField
            value={owner} onChange={setOwner}
            users={users} loading={directoryLoading} error={directoryError}
          />
        </Field>

        <div className="grid gap-4 sm:grid-cols-2">
          <Field label={t('addr_display_name', { defaultValue: 'Nom affiché' })} htmlFor="mb-name">
            <Input id="mb-name" value={name} onChange={e => setName(e.target.value)}
              placeholder={t('addr_display_name_ph', { defaultValue: 'Service commercial' })} />
          </Field>
          <Field
            label={t('addr_quota', { defaultValue: 'Quota' })}
            hint={t('addr_quota_hint', {
              defaultValue: 'Laissé à 0, le quota du domaine s’applique, ou aucun s’il n’en définit pas.',
            })}
          >
            <QuotaField bytes={quota} onChange={setQuota} />
          </Field>
        </div>

        <Field label={t('addr_comment', { defaultValue: 'Commentaire' })} htmlFor="mb-comment">
          <Textarea id="mb-comment" rows={2} value={comment}
            onChange={e => setComment(e.target.value)}
            placeholder={t('addr_comment_ph', { defaultValue: 'À quoi sert cette adresse (facultatif)' })} />
        </Field>

        <div className="rounded-lg border border-border bg-surface-1 p-3">
          <Checkbox
            checked={withCredential}
            onChange={setWithCredential}
            label={t('addr_cred_create', { defaultValue: 'Créer aussi l’identifiant IMAP/SMTP' })}
            description={t('addr_cred_create_desc', {
              defaultValue:
                'Nécessaire pour relever cette adresse depuis un client de messagerie. Le mot de passe généré ne sera affiché qu’une seule fois.',
            })}
          />
          {withCredential && (
            <div className="mt-2 pl-6">
              <Checkbox
                checked={replace}
                onChange={setReplace}
                label={t('addr_cred_replace', { defaultValue: 'Remplacer un identifiant existant' })}
                description={t('addr_cred_replace_desc', {
                  defaultValue:
                    'À n’activer que si un identifiant porte déjà cette adresse : son mot de passe changera, et le client de messagerie déjà configuré cessera de fonctionner.',
                })}
              />
            </div>
          )}
        </div>

        <div className="flex justify-end gap-2">
          <Button variant="ghost" onClick={onClose}>
            {t('cancel', { defaultValue: 'Annuler' })}
          </Button>
          <Button
            onClick={() => createMut.mutate()}
            disabled={!ready || createMut.isPending}
            loading={createMut.isPending}
          >
            {t('create', { defaultValue: 'Créer' })}
          </Button>
        </div>
      </div>
    </FloatingWindow>
  )
}

// ── The record, edited in place ──────────────────────────────────────────────

function MailboxRecord({
  mailbox, domains, users, directoryLoading, directoryError, onBack, onCredential, onDelete,
}: {
  mailbox: Mailbox
  domains: string[]
  users:   DirectoryUser[]
  directoryLoading: boolean
  directoryError:   string | null
  onBack:       () => void
  onCredential: (cred: CreatedCredential) => void
  onDelete:     () => void
}) {
  const { t } = useTranslation('mail')
  const qc = useQueryClient()
  const { confirm, confirmState, handleConfirm, handleCancel } = useConfirm()

  const split = splitAddress(mailbox.address)
  // Seeded from the record and keyed on its id by the caller, so opening
  // another mailbox starts from ITS values rather than the previous one's.
  const [local,   setLocal]   = useState(split.local)
  const [domain,  setDomain]  = useState(split.domain)
  const [owner,   setOwner]   = useState(mailbox.user_id)
  const [name,    setName]    = useState(mailbox.display_name ?? '')
  const [quota,   setQuota]   = useState(mailbox.quota_bytes)
  const [act,     setAct]     = useState(mailbox.is_active)
  const [comment, setComment] = useState(mailbox.comment ?? '')
  const [error,   setError]   = useState<string | null>(null)
  const [saved,   setSaved]   = useState(false)

  const saveMut = useMutation({
    mutationFn: () => addressesApi.updateMailbox(mailbox.id, {
      address:      joinAddress(local, domain),
      user_id:      owner,
      display_name: name.trim(),
      quota_bytes:  quota,
      is_active:    act,
      comment:      comment.trim(),
    }),
    onSuccess: async (updated) => {
      setError(null)
      setSaved(true)
      window.setTimeout(() => setSaved(false), 2500)
      // The fields are re-seeded from the SERVER's answer first, and only then
      // is the list refetched. Clearing them and waiting for the reload would
      // flash the previous values back — the race this panel is not allowed to
      // have while an administrator is reading what they just changed.
      const next = splitAddress(updated.address)
      setLocal(next.local); setDomain(next.domain)
      setOwner(updated.user_id)
      setName(updated.display_name ?? '')
      setQuota(updated.quota_bytes)
      setAct(updated.is_active)
      setComment(updated.comment ?? '')
      await qc.invalidateQueries({ queryKey: [ADDR_KEYS.mailboxes] })
      void qc.invalidateQueries({ queryKey: [ADDR_KEYS.domains] })
    },
    onError: (e) => setError(serverMessage(e, t('addr_mb_save_failed', {
      defaultValue: 'Enregistrement impossible',
    }))),
  })

  const credentialMut = useMutation({
    mutationFn: () => addressesApi.issueCredential(mailbox.id),
    onSuccess: (cred) => {
      setError(null)
      onCredential(cred)
      void qc.invalidateQueries({ queryKey: [ADDR_KEYS.mailboxes] })
    },
    onError: (e) => setError(serverMessage(e, t('addr_cred_failed', {
      defaultValue: 'Identifiant non généré',
    }))),
  })

  const askCredential = async () => {
    const ok = await confirm({
      title: mailbox.has_credential
        ? t('addr_cred_rotate_title', { defaultValue: 'Régénérer le mot de passe ?' })
        : t('addr_cred_issue_title', { defaultValue: 'Créer l’identifiant IMAP/SMTP ?' }),
      message: mailbox.has_credential
        ? t('addr_cred_rotate_msg', {
          defaultValue:
            'Un identifiant existe déjà pour « {{address}} ». Le mot de passe actuel cessera immédiatement de fonctionner : tout client de messagerie configuré avec devra être mis à jour.\n\nLe nouveau mot de passe ne sera affiché qu’une seule fois.',
          address: mailbox.address,
        })
        : t('addr_cred_issue_msg', {
          defaultValue:
            'Un identifiant permettra de relever « {{address}} » depuis un client de messagerie. Le mot de passe généré ne sera affiché qu’une seule fois.',
          address: mailbox.address,
        }),
      variant: mailbox.has_credential ? 'warning' : 'default',
      confirmLabel: t('addr_cred_go', { defaultValue: 'Générer' }),
    })
    if (ok) credentialMut.mutate()
  }

  const dirty =
    joinAddress(local, domain) !== mailbox.address ||
    owner !== mailbox.user_id ||
    name.trim() !== (mailbox.display_name ?? '') ||
    quota !== mailbox.quota_bytes ||
    act !== mailbox.is_active ||
    comment.trim() !== (mailbox.comment ?? '')

  return (
    <div className="min-w-0">
      <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
        <Button variant="ghost" size="sm" icon={<ArrowLeft size={14} />} onClick={onBack}>
          {t('addr_back_to_list', { defaultValue: 'Toutes les adresses' })}
        </Button>
        <Button variant="ghost" size="sm" icon={<Trash2 size={14} />} onClick={onDelete}>
          {t('delete', { defaultValue: 'Supprimer' })}
        </Button>
      </div>

      <ErrorNote message={error} onDismiss={() => setError(null)} />

      {mailbox.domain_served === false && (
        <Callout variant="danger" className="mb-3"
          title={t('addr_unserved_title', { defaultValue: 'Ce domaine n’est plus servi' })}>
          {t('addr_unserved_body', {
            defaultValue:
              'Le domaine « {{domain}} » ne figure plus dans « server_domains » : rien n’arrive à cette adresse. Remettez le domaine dans les réglages du serveur, ou déplacez la boîte vers un domaine servi.',
            domain: mailbox.domain,
          })}
        </Callout>
      )}
      {!mailbox.owner_known && (
        <Callout variant="warning" className="mb-3"
          title={t('addr_owner_unknown_title', { defaultValue: 'Propriétaire non reconnu' })}>
          {t('addr_owner_unknown_body', {
            defaultValue:
              'Le module n’a jamais vu ce compte : aucun message, aucun identifiant. Le courrier reçu ici sera classé dans un compte qui n’existe peut-être plus. Vérifiez le propriétaire ci-dessous.',
          })}
        </Callout>
      )}

      <div className="grid gap-4 sm:grid-cols-2">
        <Field label={t('addr_address', { defaultValue: 'Adresse' })} htmlFor="mbr-local">
          <AddressField
            id="mbr-local"
            local={local} domain={domain} domains={domains}
            onLocal={setLocal} onDomain={setDomain}
          />
        </Field>
        <Field label={t('addr_owner', { defaultValue: 'Propriétaire' })}>
          <OwnerField
            value={owner} onChange={setOwner}
            users={users} loading={directoryLoading} error={directoryError}
          />
        </Field>
        <Field label={t('addr_display_name', { defaultValue: 'Nom affiché' })} htmlFor="mbr-name">
          <Input id="mbr-name" value={name} onChange={e => setName(e.target.value)} />
        </Field>
        <Field
          label={t('addr_quota', { defaultValue: 'Quota' })}
          hint={t('addr_usage_unmeasured', {
            defaultValue: 'L’occupation réelle n’est pas encore mesurée : seul le quota configuré est connu.',
          })}
        >
          <QuotaField bytes={quota} onChange={setQuota} />
        </Field>
        <Field label={t('addr_comment', { defaultValue: 'Commentaire' })} htmlFor="mbr-comment">
          <Textarea id="mbr-comment" rows={2} value={comment}
            onChange={e => setComment(e.target.value)} />
        </Field>
        <Field label={t('addr_state', { defaultValue: 'État' })}>
          <Toggle
            checked={act}
            onChange={e => setAct(e.target.checked)}
            label={t('addr_accepts_mail', { defaultValue: 'Accepte le courrier' })}
            description={t('addr_accepts_mail_desc', {
              defaultValue: 'Suspendue, l’adresse est refusée à la réception sans être supprimée.',
            })}
          />
        </Field>
      </div>

      <div className="mt-4 flex flex-wrap items-center gap-2 rounded-lg border border-border bg-surface-1 p-3">
        <KeyRound size={16} className="text-text-secondary" />
        <span className="text-sm text-text-secondary">
          {mailbox.has_credential
            ? t('addr_cred_present', { defaultValue: 'Un identifiant IMAP/SMTP existe pour cette adresse.' })
            : t('addr_cred_absent', { defaultValue: 'Aucun identifiant IMAP/SMTP : cette adresse ne peut pas être relevée depuis un client.' })}
        </span>
        <Button variant="secondary" size="sm" onClick={() => void askCredential()}
          loading={credentialMut.isPending}>
          {mailbox.has_credential
            ? t('addr_cred_rotate', { defaultValue: 'Régénérer le mot de passe' })
            : t('addr_cred_issue', { defaultValue: 'Créer l’identifiant' })}
        </Button>
      </div>

      <div className="mt-4 flex items-center justify-end gap-2">
        {saved && (
          <span className="text-xs text-success">
            {t('saved', { defaultValue: 'Enregistré' })}
          </span>
        )}
        <Button onClick={() => saveMut.mutate()} disabled={!dirty || saveMut.isPending}
          loading={saveMut.isPending}>
          {t('save', { defaultValue: 'Enregistrer' })}
        </Button>
      </div>

      {confirmState && (
        <ConfirmDialog {...confirmState} onConfirm={handleConfirm} onCancel={handleCancel} />
      )}
    </div>
  )
}
