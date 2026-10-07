import { useEffect, useState } from 'react'
import { useSearchParams } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import {
  Mail, Plus, Trash2, RefreshCw, Loader2, AlertCircle, CheckCircle2, Server,
  Download, ExternalLink, Star, Pencil, ShieldCheck, ShieldAlert, X,
  UserPlus, Clock, Check,
} from 'lucide-react'
import { mailApi, apiErrorMessage, type EmailAccount, type SendAsAddress, type Delegation } from '../api'
import { Button, Badge, ConfirmDialog, Input } from '@ui'
import { useConfirm } from '@kubuno/sdk'
import { AccountForm } from './AccountForm'
import { SettingsRow } from './SettingsRow'

// ── Accounts and import tab ───────────────────────────────────────────────────
//
// Structured "the Gmail way" but adapted to Kubuno (self-hosted, no Google
// account): account settings live in the core, identities map onto the mail
// accounts, and external fetching maps onto external IMAP/POP3 accounts.

/** Section title separating groups of related settings (matches GeneralTab). */
function Section({ title, description }: { title: string; description?: string }) {
  return (
    <div className="mt-8 mb-1 pt-6 border-t border-[#e8eaed] first:mt-0 first:pt-0 first:border-0">
      <h3 className="text-sm font-medium text-[#202124]">{title}</h3>
      {description && (
        <p className="text-xs text-text-tertiary mt-1 leading-relaxed max-w-2xl">{description}</p>
      )}
    </div>
  )
}

export function AccountsTab() {
  const { t, i18n } = useTranslation('mail')
  const qc = useQueryClient()
  const [showForm,    setShowForm]    = useState(false)
  const [editAccount, setEditAccount] = useState<EmailAccount | null>(null)
  const { confirm, confirmState, handleConfirm, handleCancel } = useConfirm()

  // Landing back from an OAuth flow (?oauth=<provider>&status=ok|error):
  // refresh the account list, surface the outcome, and clean the URL.
  const [searchParams, setSearchParams] = useSearchParams()
  const [oauthNotice, setOauthNotice] = useState<{ ok: boolean; provider: string } | null>(null)
  useEffect(() => {
    const provider = searchParams.get('oauth')
    const status   = searchParams.get('status')
    if (!provider || !status) return
    setOauthNotice({
      ok:       status === 'ok',
      provider: provider === 'google' ? 'Google' : 'Microsoft',
    })
    qc.invalidateQueries({ queryKey: ['mail-accounts'] })
    const next = new URLSearchParams(searchParams)
    next.delete('oauth')
    next.delete('status')
    next.delete('reason')
    setSearchParams(next, { replace: true })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const { data, isLoading } = useQuery({
    queryKey: ['mail-accounts'],
    queryFn:  mailApi.listAccounts,
  })

  const deleteMut = useMutation({
    mutationFn: (id: string) => mailApi.deleteAccount(id),
    onSuccess:  () => qc.invalidateQueries({ queryKey: ['mail-accounts'] }),
  })

  const syncMut = useMutation({
    mutationFn: (id: string) => mailApi.triggerSync(id),
  })

  // Promote an identity to the default sender. Reuses updateAccount so no new
  // backend surface is required.
  const defaultMut = useMutation({
    mutationFn: (id: string) => mailApi.updateAccount(id, { is_default: true }),
    onSuccess:  () => qc.invalidateQueries({ queryKey: ['mail-accounts'] }),
  })

  // ── "Send mail as" custom addresses (server-backed, ownership-verified) ────
  // Distinct from the hosted/connected accounts above: these are addresses the
  // user claims to own, usable only once a mailed confirmation code is entered.
  const { data: sendAsList, isLoading: sendAsLoading } = useQuery({
    queryKey: ['mail-send-as'],
    queryFn:  mailApi.listSendAs,
  })
  const [showSendAsForm, setShowSendAsForm] = useState(false)
  const [saEmail, setSaEmail] = useState('')
  const [saName,  setSaName]  = useState('')
  const [saError, setSaError] = useState<string | null>(null)
  // Per-row confirmation-code entry and its inline error.
  const [codeInputs, setCodeInputs] = useState<Record<string, string>>({})
  const [rowError,   setRowError]   = useState<Record<string, string | null>>({})

  const addSendAsMut = useMutation({
    mutationFn: () => mailApi.addSendAs({
      email:       saEmail.trim(),
      displayName: saName.trim() || undefined,
    }),
    onSuccess: () => {
      setSaEmail(''); setSaName(''); setSaError(null); setShowSendAsForm(false)
      qc.invalidateQueries({ queryKey: ['mail-send-as'] })
    },
    onError: (e) => setSaError(apiErrorMessage(e, t('mail_settings_sendas_add_error', {
      defaultValue: 'Impossible d’ajouter cette adresse.',
    }))),
  })

  const resendMut = useMutation({
    mutationFn: (id: string) => mailApi.resendSendAs(id),
    onSuccess:  (_d, id) => setRowError(m => ({ ...m, [id]: null })),
    onError:    (e, id)  => setRowError(m => ({ ...m, [id]: apiErrorMessage(e, t('mail_settings_sendas_resend_error', {
      defaultValue: 'Impossible de renvoyer le code.',
    })) })),
  })

  const verifyMut = useMutation({
    mutationFn: (v: { id: string; code: string }) => mailApi.verifySendAs(v.id, v.code),
    onSuccess:  (_d, v) => {
      setRowError(m => ({ ...m, [v.id]: null }))
      setCodeInputs(c => ({ ...c, [v.id]: '' }))
      qc.invalidateQueries({ queryKey: ['mail-send-as'] })
    },
    onError:    (e, v) => setRowError(m => ({ ...m, [v.id]: apiErrorMessage(e, t('mail_settings_sendas_verify_error', {
      defaultValue: 'Code invalide ou expiré.',
    })) })),
  })

  const deleteSendAsMut = useMutation({
    mutationFn: (id: string) => mailApi.deleteSendAs(id),
    onSuccess:  () => qc.invalidateQueries({ queryKey: ['mail-send-as'] }),
  })

  async function askDeleteSendAs(addr: SendAsAddress) {
    const ok = await confirm({
      title:        t('mail_settings_sendas_delete_title', { defaultValue: 'Retirer cette adresse ?' }),
      message:      t('mail_settings_sendas_delete_msg', {
        defaultValue: 'L’adresse « {{email}} » sera retirée de vos expéditeurs.',
        email: addr.email,
      }),
      confirmLabel: t('common_delete', { defaultValue: 'Supprimer' }),
      cancelLabel:  t('common_cancel', { defaultValue: 'Annuler' }),
      variant:      'danger',
    })
    if (ok) deleteSendAsMut.mutate(addr.id)
  }

  const sendAsAddresses = sendAsList ?? []

  // ── Account delegation (server-backed, ownership-enforced) ─────────────────
  // Two views share the mail.delegations table: delegations I GRANTED (as the
  // mailbox owner) and delegations INCOMING to me (accounts I may act on).
  const { data: delegations, isLoading: delegLoading } = useQuery({
    queryKey: ['mail-delegations'],
    queryFn:  mailApi.listDelegations,
  })
  const { data: incoming } = useQuery({
    queryKey: ['mail-delegations-incoming'],
    queryFn:  mailApi.listIncomingDelegations,
  })

  const [showDelegForm, setShowDelegForm] = useState(false)
  const [delegEmail, setDelegEmail] = useState('')
  const [delegError, setDelegError] = useState<string | null>(null)

  const addDelegMut = useMutation({
    mutationFn: () => mailApi.addDelegation(delegEmail.trim()),
    onSuccess: () => {
      setDelegEmail(''); setDelegError(null); setShowDelegForm(false)
      qc.invalidateQueries({ queryKey: ['mail-delegations'] })
    },
    onError: (e) => setDelegError(apiErrorMessage(e, t('mail_settings_delegate_add_error', {
      defaultValue: 'Impossible d’accorder l’accès à cette adresse.',
    }))),
  })

  const revokeDelegMut = useMutation({
    mutationFn: (id: string) => mailApi.revokeDelegation(id),
    onSuccess:  () => qc.invalidateQueries({ queryKey: ['mail-delegations'] }),
  })

  const acceptDelegMut = useMutation({
    mutationFn: (id: string) => mailApi.acceptDelegation(id),
    onSuccess:  () => qc.invalidateQueries({ queryKey: ['mail-delegations-incoming'] }),
  })

  const declineDelegMut = useMutation({
    mutationFn: (id: string) => mailApi.declineDelegation(id),
    onSuccess:  () => qc.invalidateQueries({ queryKey: ['mail-delegations-incoming'] }),
  })

  async function askRevokeDelegation(d: Delegation) {
    const ok = await confirm({
      title:        t('mail_settings_delegate_revoke_title', { defaultValue: 'Révoquer cet accès ?' }),
      message:      t('mail_settings_delegate_revoke_msg', {
        defaultValue: '« {{email}} » n’aura plus accès à votre boîte de réception.',
        email: d.delegateEmail,
      }),
      confirmLabel: t('mail_settings_delegate_revoke_confirm', { defaultValue: 'Révoquer' }),
      cancelLabel:  t('common_cancel', { defaultValue: 'Annuler' }),
      variant:      'danger',
    })
    if (ok) revokeDelegMut.mutate(d.id)
  }

  // Only the live delegations (pending/accepted) are shown as "granted"; a
  // revoked row is history and would only clutter the list.
  const grantedDelegations = (delegations ?? []).filter(d => d.status !== 'revoked')
  const incomingDelegations = incoming ?? []

  const accounts = data?.accounts ?? []
  const externalAccounts = accounts.filter(a => a.kind !== 'local')

  const openAdd  = () => { setEditAccount(null); setShowForm(true) }
  const openEdit = (a: EmailAccount) => { setEditAccount(a); setShowForm(true) }

  // Deletion is confirmed through the core dialog — browser dialogs are banned.
  async function askDelete(a: EmailAccount) {
    if (a.kind === 'local') return
    const ok = await confirm({
      title:        t('mail_settings_delete_account_title', { defaultValue: 'Supprimer ce compte ?' }),
      message:      t('mail_settings_delete_account_msg', {
        defaultValue: 'Le compte « {{email}} » sera retiré. Les messages déjà synchronisés seront supprimés de cette instance.',
        email: a.email_address,
      }),
      confirmLabel: t('common_delete', { defaultValue: 'Supprimer' }),
      cancelLabel:  t('common_cancel', { defaultValue: 'Annuler' }),
      variant:      'danger',
    })
    if (ok) deleteMut.mutate(a.id)
  }

  return (
    <div>
      {oauthNotice && (
        <div
          className={`mb-4 flex items-center gap-2 text-sm px-3 py-2 rounded-lg ${
            oauthNotice.ok ? 'bg-success/10 text-success' : 'bg-danger/10 text-danger'
          }`}
        >
          {oauthNotice.ok ? <CheckCircle2 size={14} /> : <AlertCircle size={14} />}
          {oauthNotice.ok
            ? t('mail_oauth_success', { provider: oauthNotice.provider })
            : t('mail_oauth_error')}
        </div>
      )}

      {/* ── Account settings (handled by the core) ─────────────────────────── */}
      <Section
        title={t('mail_settings_account_settings', { defaultValue: 'Modifier les paramètres du compte' })}
      />
      <SettingsRow
        label={t('mail_settings_account_change_password', { defaultValue: 'Mot de passe' })}
        description={t('mail_settings_account_change_password_desc', {
          defaultValue: 'Votre compte et votre mot de passe sont gérés par cette instance Kubuno.',
        })}
      >
        <a
          href="/settings?tab=security"
          className="inline-flex items-center gap-1.5 text-sm text-primary hover:underline"
        >
          <ExternalLink size={13} />
          {t('mail_settings_account_change_password_link', { defaultValue: 'Changer le mot de passe' })}
        </a>
      </SettingsRow>
      <SettingsRow
        label={t('mail_settings_account_profile', { defaultValue: 'Profil' })}
        description={t('mail_settings_account_profile_desc', {
          defaultValue: 'Nom affiché, avatar et informations personnelles.',
        })}
      >
        <a
          href="/settings?tab=profile"
          className="inline-flex items-center gap-1.5 text-sm text-primary hover:underline"
        >
          <ExternalLink size={13} />
          {t('mail_settings_account_profile_link', { defaultValue: 'Modifier le profil' })}
        </a>
      </SettingsRow>

      {/* ── Import mail and contacts ───────────────────────────────────────── */}
      <Section
        title={t('mail_settings_import_section', { defaultValue: 'Importation du courrier et des contacts' })}
      />
      <SettingsRow
        label={t('mail_settings_import_label', { defaultValue: 'Importer depuis un autre compte' })}
        description={t('mail_settings_import_desc', {
          defaultValue: 'Connectez un compte IMAP, POP3 ou OAuth (Gmail, Microsoft) pour récupérer son courrier dans Kubuno.',
        })}
      >
        <Button size="sm" variant="secondary" icon={<Download size={14} />} onClick={openAdd}>
          {t('mail_settings_import_action', { defaultValue: 'Importer le courrier et les contacts' })}
        </Button>
      </SettingsRow>

      {/* ── Send mail as (identities) ──────────────────────────────────────── */}
      <Section
        title={t('mail_settings_sendas_section', { defaultValue: 'Envoyer des e-mails en tant que' })}
        description={t('mail_settings_sendas_desc', {
          defaultValue: 'Choisissez le nom et l’adresse qui apparaissent lorsque vous rédigez un message.',
        })}
      />
      {isLoading ? (
        <div className="flex justify-center py-6">
          <Loader2 size={18} className="animate-spin text-text-tertiary" />
        </div>
      ) : accounts.length === 0 ? (
        <p className="text-sm text-text-tertiary py-4">
          {t('mail_settings_sendas_empty', { defaultValue: 'Aucune adresse configurée pour le moment.' })}
        </p>
      ) : (
        <div className="divide-y divide-[#e8eaed]">
          {accounts.map(account => (
            <div key={account.id} className="flex items-center justify-between gap-3 py-3">
              <div className="min-w-0">
                <div className="flex items-center gap-2 flex-wrap">
                  <span className="text-sm text-text-primary font-medium truncate">{account.name}</span>
                  <span className="text-sm text-text-secondary truncate">&lt;{account.email_address}&gt;</span>
                  {account.is_default && (
                    <Badge variant="primary" size="sm">
                      {t('mail_settings_badge_default', { defaultValue: 'Par défaut' })}
                    </Badge>
                  )}
                  {account.kind === 'local' && (
                    <Badge variant="neutral" size="sm">
                      <Server size={11} />
                      {t('mail_settings_badge_hosted', { defaultValue: 'Hébergé par cette instance' })}
                    </Badge>
                  )}
                </div>
              </div>
              <div className="flex items-center gap-1 flex-shrink-0">
                {!account.is_default && (
                  <button
                    onClick={() => defaultMut.mutate(account.id)}
                    disabled={defaultMut.isPending}
                    className="inline-flex items-center gap-1 px-2 py-1 rounded-lg text-text-secondary hover:bg-surface-2 text-xs font-medium"
                  >
                    <Star size={12} />
                    {t('mail_settings_make_default', { defaultValue: 'Par défaut' })}
                  </button>
                )}
                <button
                  onClick={() => openEdit(account)}
                  className="inline-flex items-center gap-1 px-2 py-1 rounded-lg text-text-secondary hover:bg-surface-2 text-xs font-medium"
                >
                  <Pencil size={12} />
                  {t('common_edit', { defaultValue: 'Modifier' })}
                </button>
                {account.kind !== 'local' && (
                  <button
                    onClick={() => askDelete(account)}
                    className="inline-flex items-center gap-1 px-2 py-1 rounded-lg text-text-tertiary hover:text-danger hover:bg-danger/10 text-xs font-medium"
                  >
                    <Trash2 size={12} />
                    {t('common_delete', { defaultValue: 'Supprimer' })}
                  </button>
                )}
              </div>
            </div>
          ))}
        </div>
      )}
      {/* ── Custom "send as" addresses (ownership-verified) ──────────────────
          Adding one mails a confirmation code to the address; it becomes usable
          only once that code is entered here (Gmail's flow). */}
      {sendAsLoading ? (
        <div className="flex justify-center py-4">
          <Loader2 size={16} className="animate-spin text-text-tertiary" />
        </div>
      ) : sendAsAddresses.length > 0 && (
        <div className="divide-y divide-[#e8eaed] mt-1">
          {sendAsAddresses.map(addr => (
            <div key={addr.id} className="py-3">
              <div className="flex items-center justify-between gap-3">
                <div className="min-w-0">
                  <div className="flex items-center gap-2 flex-wrap">
                    {addr.displayName && (
                      <span className="text-sm text-text-primary font-medium truncate">{addr.displayName}</span>
                    )}
                    <span className="text-sm text-text-secondary truncate">&lt;{addr.email}&gt;</span>
                    {addr.verified ? (
                      <Badge variant="success" size="sm">
                        <ShieldCheck size={11} />
                        {t('mail_settings_sendas_verified', { defaultValue: 'Vérifiée' })}
                      </Badge>
                    ) : (
                      <Badge variant="warning" size="sm">
                        <ShieldAlert size={11} />
                        {t('mail_settings_sendas_unverified', { defaultValue: 'Non vérifiée' })}
                      </Badge>
                    )}
                  </div>
                </div>
                <div className="flex items-center gap-1 flex-shrink-0">
                  <button
                    onClick={() => askDeleteSendAs(addr)}
                    className="inline-flex items-center gap-1 px-2 py-1 rounded-lg text-text-tertiary hover:text-danger hover:bg-danger/10 text-xs font-medium"
                  >
                    <Trash2 size={12} />
                    {t('common_delete', { defaultValue: 'Supprimer' })}
                  </button>
                </div>
              </div>

              {/* Verification controls, only while the address is unconfirmed. */}
              {!addr.verified && (
                <div className="mt-2 pl-1">
                  <p className="text-xs text-text-tertiary mb-2 max-w-2xl">
                    {t('mail_settings_sendas_pending', {
                      defaultValue: 'Un code de confirmation a été envoyé à cette adresse. Saisissez-le ci-dessous pour l’activer.',
                    })}
                  </p>
                  <div className="flex flex-wrap items-center gap-2">
                    <div className="w-40">
                      <Input
                        value={codeInputs[addr.id] ?? ''}
                        onChange={e => setCodeInputs(c => ({ ...c, [addr.id]: e.target.value }))}
                        placeholder={t('mail_settings_sendas_code_placeholder', { defaultValue: 'Code reçu' })}
                      />
                    </div>
                    <Button
                      size="sm"
                      variant="primary"
                      disabled={!((codeInputs[addr.id] ?? '').trim()) || verifyMut.isPending}
                      onClick={() => verifyMut.mutate({ id: addr.id, code: (codeInputs[addr.id] ?? '').trim() })}
                    >
                      {t('mail_settings_sendas_verify', { defaultValue: 'Vérifier' })}
                    </Button>
                    <button
                      onClick={() => resendMut.mutate(addr.id)}
                      disabled={resendMut.isPending}
                      className="inline-flex items-center gap-1 text-sm text-primary hover:underline disabled:opacity-50"
                    >
                      <RefreshCw size={12} className={resendMut.isPending ? 'animate-spin' : ''} />
                      {t('mail_settings_sendas_resend', { defaultValue: 'Renvoyer le code' })}
                    </button>
                  </div>
                  {rowError[addr.id] && (
                    <div className="flex items-center gap-1 mt-1.5 text-xs text-danger">
                      <AlertCircle size={11} />
                      {rowError[addr.id]}
                    </div>
                  )}
                </div>
              )}
            </div>
          ))}
        </div>
      )}

      {/* Add-a-send-as-address inline form (name + email → mails a code). */}
      <div className="pt-3">
        {showSendAsForm ? (
          <div className="border border-border rounded-xl p-4 bg-white max-w-lg">
            <div className="flex items-center justify-between mb-3">
              <p className="text-sm font-medium text-text-primary">
                {t('mail_settings_sendas_add_title', { defaultValue: 'Ajouter une adresse d’envoi' })}
              </p>
              <button
                onClick={() => { setShowSendAsForm(false); setSaError(null) }}
                className="p-1 rounded-lg text-text-tertiary hover:bg-surface-2"
              >
                <X size={14} />
              </button>
            </div>
            <div className="space-y-2">
              <Input
                value={saName}
                onChange={e => setSaName(e.target.value)}
                placeholder={t('mail_settings_sendas_name_placeholder', { defaultValue: 'Nom affiché (facultatif)' })}
              />
              <Input
                type="email"
                value={saEmail}
                onChange={e => setSaEmail(e.target.value)}
                placeholder={t('mail_settings_sendas_email_placeholder', { defaultValue: 'adresse@exemple.com' })}
              />
            </div>
            {saError && (
              <div className="flex items-center gap-1 mt-2 text-xs text-danger">
                <AlertCircle size={11} />
                {saError}
              </div>
            )}
            <p className="text-xs text-text-tertiary mt-2">
              {t('mail_settings_sendas_add_hint', {
                defaultValue: 'Un code de confirmation sera envoyé à cette adresse pour vérifier que vous en êtes propriétaire.',
              })}
            </p>
            <div className="flex items-center gap-2 mt-3">
              <Button
                size="sm"
                variant="primary"
                disabled={!saEmail.trim() || addSendAsMut.isPending}
                onClick={() => addSendAsMut.mutate()}
              >
                {addSendAsMut.isPending
                  ? t('mail_settings_sendas_sending', { defaultValue: 'Envoi du code…' })
                  : t('mail_settings_sendas_send_code', { defaultValue: 'Envoyer le code de confirmation' })}
              </Button>
              <Button
                size="sm"
                variant="secondary"
                onClick={() => { setShowSendAsForm(false); setSaError(null) }}
              >
                {t('common_cancel', { defaultValue: 'Annuler' })}
              </Button>
            </div>
          </div>
        ) : (
          <button
            onClick={() => setShowSendAsForm(true)}
            className="inline-flex items-center gap-1.5 text-sm text-primary hover:underline"
          >
            <Plus size={14} />
            {t('mail_settings_sendas_add', { defaultValue: 'Ajouter une autre adresse e-mail' })}
          </button>
        )}
      </div>

      {/* ── Check mail from other accounts (external fetching) ──────────────── */}
      <Section
        title={t('mail_settings_check_section', { defaultValue: 'Consulter d’autres comptes de messagerie' })}
        description={t('mail_settings_check_desc', {
          defaultValue: 'Comptes externes dont Kubuno récupère le courrier par IMAP ou POP3.',
        })}
      />
      {isLoading ? (
        <div className="flex justify-center py-6">
          <Loader2 size={18} className="animate-spin text-text-tertiary" />
        </div>
      ) : externalAccounts.length === 0 ? (
        <div className="text-center py-8">
          <Mail size={28} className="opacity-30 mx-auto mb-2 text-text-tertiary" />
          <p className="text-sm text-text-tertiary font-medium">
            {t('mail_settings_no_external', { defaultValue: 'Aucun compte externe connecté' })}
          </p>
        </div>
      ) : (
        <div className="space-y-3 py-2">
          {externalAccounts.map(account => (
            <div
              key={account.id}
              className="border border-border rounded-xl p-4 hover:shadow-sm transition-shadow bg-white"
            >
              <div className="flex items-start justify-between gap-3">
                <div className="flex-1 min-w-0">
                  <div className="flex items-center gap-2 mb-1 flex-wrap">
                    <p className="font-medium text-text-primary text-sm">{account.name}</p>
                    {!account.is_active && (
                      <span className="text-xs bg-warning/10 text-warning px-1.5 py-0.5 rounded-full">
                        {t('mail_settings_badge_inactive')}
                      </span>
                    )}
                    {account.auth_kind?.startsWith('oauth') && (
                      <span className="text-xs bg-primary/10 text-primary px-1.5 py-0.5 rounded-full">
                        {t('mail_oauth_badge')}
                      </span>
                    )}
                  </div>
                  <p className="text-sm text-text-secondary">{account.email_address}</p>
                  <p className="text-xs text-text-tertiary mt-1">
                    {account.incoming_protocol?.toUpperCase() ?? 'IMAP'}: {account.imap_host}:{account.imap_port} ·{' '}
                    SMTP: {account.smtp_host}:{account.smtp_port}
                  </p>
                  {account.last_sync_at && (
                    <p className="text-xs text-text-tertiary mt-0.5">
                      {t('mail_settings_last_sync')}{' '}
                      {new Date(account.last_sync_at).toLocaleString(i18n.language)}
                    </p>
                  )}
                  {account.last_error && (
                    <div className="flex items-center gap-1 mt-1 text-xs text-danger">
                      <AlertCircle size={11} />
                      {account.last_error}
                    </div>
                  )}
                </div>
                <div className="flex items-center gap-1">
                  <button
                    onClick={() => syncMut.mutate(account.id)}
                    disabled={syncMut.isPending}
                    title={t('mail_settings_sync_now')}
                    className="p-1.5 rounded-lg text-text-tertiary hover:bg-surface-2"
                  >
                    <RefreshCw size={14} className={syncMut.isPending ? 'animate-spin' : ''} />
                  </button>
                  <button
                    onClick={() => openEdit(account)}
                    className="px-2 py-1.5 rounded-lg text-text-secondary hover:bg-surface-2 text-xs font-medium"
                  >
                    {t('common_edit')}
                  </button>
                  <button
                    onClick={() => askDelete(account)}
                    disabled={deleteMut.isPending}
                    className="p-1.5 rounded-lg text-text-tertiary hover:text-danger hover:bg-danger/10 disabled:opacity-40"
                  >
                    <Trash2 size={14} />
                  </button>
                </div>
              </div>
            </div>
          ))}
        </div>
      )}
      <div className="pt-3">
        <button
          onClick={openAdd}
          className="inline-flex items-center gap-1.5 text-sm text-primary hover:underline"
        >
          <Plus size={14} />
          {t('mail_settings_check_add', { defaultValue: 'Ajouter un compte de messagerie' })}
        </button>
      </div>

      {/* ── Grant access to your account (delegation) ──────────────────────── */}
      <Section
        title={t('mail_settings_delegate_section', { defaultValue: 'Déléguer l’accès à votre compte' })}
        description={t('mail_settings_delegate_desc', {
          defaultValue: 'Autorisez un autre utilisateur de cette instance à lire votre courrier et à envoyer des messages en votre nom, sans partager votre mot de passe. La personne doit accepter l’invitation ; vous pouvez révoquer l’accès à tout moment.',
        })}
      />
      {delegLoading ? (
        <div className="flex justify-center py-6">
          <Loader2 size={18} className="animate-spin text-text-tertiary" />
        </div>
      ) : grantedDelegations.length === 0 ? (
        <p className="text-sm text-text-tertiary py-4">
          {t('mail_settings_delegate_empty', { defaultValue: 'Vous n’avez accordé l’accès à personne pour le moment.' })}
        </p>
      ) : (
        <div className="divide-y divide-[#e8eaed]">
          {grantedDelegations.map(d => (
            <div key={d.id} className="flex items-center justify-between gap-3 py-3">
              <div className="min-w-0">
                <div className="flex items-center gap-2 flex-wrap">
                  <span className="text-sm text-text-secondary truncate">{d.delegateEmail}</span>
                  {d.status === 'accepted' ? (
                    <Badge variant="success" size="sm">
                      <ShieldCheck size={11} />
                      {t('mail_settings_delegate_accepted', { defaultValue: 'Accès accordé' })}
                    </Badge>
                  ) : (
                    <Badge variant="warning" size="sm">
                      <Clock size={11} />
                      {t('mail_settings_delegate_pending', { defaultValue: 'En attente d’acceptation' })}
                    </Badge>
                  )}
                </div>
              </div>
              <button
                onClick={() => askRevokeDelegation(d)}
                disabled={revokeDelegMut.isPending}
                className="inline-flex items-center gap-1 px-2 py-1 rounded-lg text-text-tertiary hover:text-danger hover:bg-danger/10 text-xs font-medium flex-shrink-0"
              >
                <Trash2 size={12} />
                {t('mail_settings_delegate_revoke', { defaultValue: 'Révoquer' })}
              </button>
            </div>
          ))}
        </div>
      )}

      {/* Add-a-delegate inline form (email → invitation the person must accept). */}
      <div className="pt-3">
        {showDelegForm ? (
          <div className="border border-border rounded-xl p-4 bg-white max-w-lg">
            <div className="flex items-center justify-between mb-3">
              <p className="text-sm font-medium text-text-primary">
                {t('mail_settings_delegate_add_title', { defaultValue: 'Ajouter un délégué' })}
              </p>
              <button
                onClick={() => { setShowDelegForm(false); setDelegError(null) }}
                className="p-1 rounded-lg text-text-tertiary hover:bg-surface-2"
              >
                <X size={14} />
              </button>
            </div>
            <Input
              type="email"
              value={delegEmail}
              onChange={e => setDelegEmail(e.target.value)}
              placeholder={t('mail_settings_delegate_email_placeholder', { defaultValue: 'adresse@exemple.com' })}
            />
            {delegError && (
              <div className="flex items-center gap-1 mt-2 text-xs text-danger">
                <AlertCircle size={11} />
                {delegError}
              </div>
            )}
            <p className="text-xs text-text-tertiary mt-2">
              {t('mail_settings_delegate_add_hint', {
                defaultValue: 'La personne recevra une invitation qu’elle devra accepter avant d’accéder à votre compte.',
              })}
            </p>
            <div className="flex items-center gap-2 mt-3">
              <Button
                size="sm"
                variant="primary"
                disabled={!delegEmail.trim() || addDelegMut.isPending}
                onClick={() => addDelegMut.mutate()}
              >
                {addDelegMut.isPending
                  ? t('mail_settings_delegate_adding', { defaultValue: 'Envoi de l’invitation…' })
                  : t('mail_settings_delegate_send_invite', { defaultValue: 'Accorder l’accès' })}
              </Button>
              <Button
                size="sm"
                variant="secondary"
                onClick={() => { setShowDelegForm(false); setDelegError(null) }}
              >
                {t('common_cancel', { defaultValue: 'Annuler' })}
              </Button>
            </div>
          </div>
        ) : (
          <button
            onClick={() => setShowDelegForm(true)}
            className="inline-flex items-center gap-1.5 text-sm text-primary hover:underline"
          >
            <UserPlus size={14} />
            {t('mail_settings_delegate_add', { defaultValue: 'Ajouter un délégué' })}
          </button>
        )}
      </div>

      {/* ── Accounts delegated to me (incoming) ────────────────────────────── */}
      {incomingDelegations.length > 0 && (
        <>
          <Section
            title={t('mail_settings_delegate_incoming_section', { defaultValue: 'Comptes auxquels vous avez accès' })}
            description={t('mail_settings_delegate_incoming_desc', {
              defaultValue: 'Boîtes d’autres utilisateurs qui vous ont accordé l’accès. Une fois l’invitation acceptée, vous pouvez lire leur courrier et envoyer en leur nom.',
            })}
          />
          <div className="divide-y divide-[#e8eaed]">
            {incomingDelegations.map(d => (
              <div key={d.id} className="flex items-center justify-between gap-3 py-3">
                <div className="min-w-0 flex items-center gap-2 flex-wrap">
                  <span className="text-sm text-text-secondary truncate">{d.grantorEmail}</span>
                  {d.status === 'accepted' ? (
                    <>
                      <Badge variant="success" size="sm">
                        <Check size={11} />
                        {t('mail_settings_delegate_incoming_active', { defaultValue: 'Actif' })}
                      </Badge>
                      {!d.canSend && (
                        <Badge variant="neutral" size="sm">
                          {t('mail_settings_delegate_incoming_readonly', { defaultValue: 'Lecture seule' })}
                        </Badge>
                      )}
                    </>
                  ) : (
                    <Badge variant="warning" size="sm">
                      <Clock size={11} />
                      {t('mail_settings_delegate_incoming_invited', { defaultValue: 'Invitation reçue' })}
                    </Badge>
                  )}
                </div>
                <div className="flex items-center gap-1 flex-shrink-0">
                  {d.status === 'pending' && (
                    <Button
                      size="sm"
                      variant="primary"
                      disabled={acceptDelegMut.isPending}
                      onClick={() => acceptDelegMut.mutate(d.id)}
                    >
                      {t('mail_settings_delegate_incoming_accept', { defaultValue: 'Accepter' })}
                    </Button>
                  )}
                  <button
                    onClick={() => declineDelegMut.mutate(d.id)}
                    disabled={declineDelegMut.isPending}
                    className="inline-flex items-center gap-1 px-2 py-1 rounded-lg text-text-tertiary hover:text-danger hover:bg-danger/10 text-xs font-medium"
                  >
                    <X size={12} />
                    {d.status === 'pending'
                      ? t('mail_settings_delegate_incoming_decline', { defaultValue: 'Refuser' })
                      : t('mail_settings_delegate_incoming_leave', { defaultValue: 'Quitter' })}
                  </button>
                </div>
              </div>
            ))}
          </div>
        </>
      )}

      {/* ── Storage usage ──────────────────────────────────────────────────── */}
      <Section
        title={t('mail_settings_storage_section', { defaultValue: 'Espace de stockage' })}
      />
      <SettingsRow
        label={t('mail_settings_storage_label', { defaultValue: 'Utilisation' })}
        description={t('mail_settings_storage_desc', {
          defaultValue: 'L’espace de stockage est géré par cette instance Kubuno. Aucune limite n’est appliquée par le module Mail.',
        })}
      >
        {/* No per-user mail-usage endpoint exists on the module yet; usage is
            declared to the core through the shared storage-usage channel, not
            exposed here. A neutral statement is shown until an API lands. */}
        <span className="text-sm text-text-tertiary">
          {t('mail_settings_storage_managed', { defaultValue: 'Géré par l’administrateur de l’instance' })}
        </span>
      </SettingsRow>

      {showForm && (
        <AccountForm
          existing={editAccount ?? undefined}
          onClose={() => { setShowForm(false); setEditAccount(null) }}
        />
      )}

      {confirmState && (
        <ConfirmDialog {...confirmState} onConfirm={handleConfirm} onCancel={handleCancel} />
      )}
    </div>
  )
}
