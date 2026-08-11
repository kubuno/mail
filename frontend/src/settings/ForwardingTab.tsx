import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Plus, Trash2, Check, Copy, Server } from 'lucide-react'
import { Button, Input, Dropdown, Checkbox } from '@ui'
import { SettingsRow, RadioGroup } from './SettingsRow'
import { mailApi, apiErrorMessage, type PopImapSettings } from '../api'
import {
  loadForwardingPrefs, saveForwardingPrefs, newForwardId,
  type ForwardingPrefs, type ForwardAddress,
} from './forwardingPrefs'

/**
 * Server-compatible starting point for the POP/IMAP policy, shown only for the
 * instant before the real values load from `/mail/pop-imap`. Mirrors the
 * server's compatibility defaults so there is no flash of a wrong state.
 */
const INITIAL_POP_IMAP: PopImapSettings = {
  imapEnabled:     true,
  imapExpunge:     'wait',
  imapPurge:       'trash',
  imapFolderLimit: '0',
  popState:        'all',
  popOnFetch:      'mark_read',
}

// ── Forwarding, POP & IMAP tab ────────────────────────────────────────────────
//
// Gmail's "Forwarding and POP/IMAP", adapted to a self-hosted instance: Kubuno
// runs its OWN SMTP/IMAP/POP3 services, so "Configure your client" points at
// this instance and reuses the mailbox-credential flow (the "Accès client" tab)
// for the dedicated password.
//
// The FORWARDING rules (destinations + keep/archive) live on the server — they
// must, so incoming mail can be re-sent while the user is offline — and are
// loaded/saved through /mail/forwarding. The POP/IMAP fetch & deletion policies
// still only live in localStorage (see forwardingPrefs.ts); those remain flagged
// inline via Callout until the mail server honours them.

/** Section title separating groups of related settings (matches GeneralTab). */
function Section({ title }: { title: string }) {
  return (
    <h3 className="text-sm font-medium text-[#202124] mt-8 mb-1 pt-6 border-t border-[#e8eaed] first:mt-0 first:pt-0 first:border-0">
      {title}
    </h3>
  )
}

/** RFC-lite address check — good enough to reject obvious typos before storing. */
function isEmail(v: string): boolean {
  return /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(v.trim())
}

/**
 * "Configure your client" — the instance's own connection facts.
 *
 * The administrator decides which services run and on which ports (see
 * MailboxAccessTab), so we surface the well-known defaults and the current host
 * rather than pretending to know the exact configuration. The password is NOT
 * shown here: it is generated once from the "Accès client" tab.
 */
function ConnectionInfo({ protocol }: { protocol: 'pop' | 'imap' }) {
  const { t } = useTranslation('mail')
  const [copied, setCopied] = useState(false)
  const host = typeof window !== 'undefined' ? window.location.hostname : 'mail.example.com'

  const rows: { label: string; value: string; copyable?: boolean }[] = [
    { label: t('mail_fwd_conn_incoming', { defaultValue: 'Serveur entrant' }),
      copyable: true,
      value: protocol === 'imap'
        ? `${host} · IMAP · 993 (SSL/TLS) ${t('mail_fwd_conn_or', { defaultValue: 'ou' })} 143 (STARTTLS)`
        : `${host} · POP3 · 995 (SSL/TLS)` },
    { label: t('mail_fwd_conn_outgoing', { defaultValue: 'Serveur sortant' }),
      value: `${host} · SMTP · 465 (SSL/TLS) ${t('mail_fwd_conn_or', { defaultValue: 'ou' })} 587 (STARTTLS)` },
    { label: t('mail_fwd_conn_username', { defaultValue: 'Identifiant' }),
      value: t('mail_fwd_conn_username_val', { defaultValue: 'Votre adresse e-mail complète' }) },
    { label: t('mail_fwd_conn_password', { defaultValue: 'Mot de passe' }),
      value: t('mail_fwd_conn_password_val', { defaultValue: 'Un mot de passe dédié — générez-le dans l’onglet « Accès client »' }) },
  ]

  const copyHost = async () => {
    try {
      await navigator.clipboard.writeText(host)
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    } catch { /* clipboard unavailable */ }
  }

  return (
    <div className="rounded-lg border border-[#e8eaed] p-4 space-y-3 max-w-xl">
      <div className="flex items-center gap-2">
        <Server size={16} className="text-text-secondary" />
        <h4 className="text-sm font-bold text-text-primary">
          {t('mail_fwd_configure_client', { defaultValue: 'Configurez votre client de messagerie' })}
        </h4>
      </div>
      <p className="text-xs text-text-secondary">
        {t('mail_fwd_configure_client_intro', {
          defaultValue:
            'Cette instance héberge ses propres services de messagerie. Utilisez ces paramètres dans Thunderbird, Outlook ou votre téléphone. Les ports exacts dépendent de la configuration de l’administrateur.',
        })}
      </p>
      <dl className="space-y-2">
        {rows.map(r => (
          <div key={r.label} className="flex flex-col sm:flex-row sm:items-baseline gap-0.5 sm:gap-3">
            <dt className="w-40 flex-shrink-0 text-xs text-text-tertiary">{r.label}</dt>
            <dd className="text-sm text-text-primary flex items-center gap-2">
              <span>{r.value}</span>
              {r.copyable && (
                <button
                  onClick={copyHost}
                  className="text-text-tertiary hover:text-primary"
                  title={t('mail_fwd_copy_host', { defaultValue: 'Copier l’adresse du serveur' })}
                >
                  {copied ? <Check size={13} /> : <Copy size={13} />}
                </button>
              )}
            </dd>
          </div>
        ))}
      </dl>
    </div>
  )
}

export function ForwardingTab() {
  const { t } = useTranslation('mail')
  const [prefs, setPrefs] = useState<ForwardingPrefs>(loadForwardingPrefs)
  const [popImap, setPopImap] = useState<PopImapSettings>(INITIAL_POP_IMAP)
  const [saved, setSaved] = useState(false)
  const [saveError, setSaveError] = useState('')
  const [newAddr, setNewAddr] = useState('')
  const [addrError, setAddrError] = useState<string | null>(null)

  // Both halves live on the server: the forwarding rules (so mail is re-sent
  // while the user is offline) and the POP/IMAP policy (enforced by the
  // instance's own IMAP/POP3 server). Load both on mount; the forwarding blob is
  // merged over its localStorage fallback, the POP/IMAP policy replaces the
  // starting default.
  useEffect(() => {
    let cancelled = false
    mailApi.getForwarding()
      .then(cfg => {
        if (cancelled) return
        setPrefs(p => ({
          ...p,
          forwardAddresses: cfg.forwardAddresses.map(a => ({
            id: newForwardId(), email: a.email, enabled: a.enabled,
          })),
          forwardKeep: cfg.forwardKeep,
        }))
      })
      .catch(() => { /* keep the localStorage value if the server is unreachable */ })
    mailApi.getPopImap()
      .then(cfg => { if (!cancelled) setPopImap(cfg) })
      .catch(() => { /* keep the default until the server is reachable */ })
    return () => { cancelled = true }
  }, [])

  const set = <K extends keyof ForwardingPrefs>(key: K, value: ForwardingPrefs[K]) =>
    setPrefs(p => ({ ...p, [key]: value }))

  const setPi = <K extends keyof PopImapSettings>(key: K, value: PopImapSettings[K]) =>
    setPopImap(p => ({ ...p, [key]: value }))

  const addForward = () => {
    const email = newAddr.trim().toLowerCase()
    if (!isEmail(email)) {
      setAddrError(t('mail_fwd_invalid_address', { defaultValue: 'Adresse e-mail invalide.' }))
      return
    }
    if (prefs.forwardAddresses.some(a => a.email === email)) {
      setAddrError(t('mail_fwd_duplicate_address', { defaultValue: 'Cette adresse est déjà dans la liste.' }))
      return
    }
    const entry: ForwardAddress = { id: newForwardId(), email, enabled: true }
    setPrefs(p => ({ ...p, forwardAddresses: [...p.forwardAddresses, entry] }))
    setNewAddr('')
    setAddrError(null)
  }

  const toggleForward = (id: string, enabled: boolean) =>
    setPrefs(p => ({
      ...p,
      forwardAddresses: p.forwardAddresses.map(a => (a.id === id ? { ...a, enabled } : a)),
    }))

  const removeForward = (id: string) =>
    setPrefs(p => ({ ...p, forwardAddresses: p.forwardAddresses.filter(a => a.id !== id) }))

  const save = async () => {
    setSaveError('')
    // Persist to the server first: a rejected save (an invalid address, or a bad
    // enum) must not be reported as saved. Forwarding rules and the POP/IMAP
    // policy are two endpoints; save both before claiming success.
    try {
      await mailApi.saveForwarding({
        forwardAddresses: prefs.forwardAddresses.map(a => ({ email: a.email, enabled: a.enabled })),
        forwardKeep: prefs.forwardKeep,
      })
      await mailApi.savePopImap(popImap)
    } catch (e) {
      setSaveError(apiErrorMessage(e, t('mail_fwd_save_error', {
        defaultValue: "Les modifications n'ont pas pu être enregistrées.",
      })))
      return
    }
    saveForwardingPrefs(prefs)
    setSaved(true)
    setTimeout(() => setSaved(false), 2500)
  }

  return (
    <div className="max-w-3xl">
      {/* ── Forwarding ─────────────────────────────────────────────────── */}
      <Section title={t('mail_fwd_section_forwarding', { defaultValue: 'Transfert' })} />

      <SettingsRow
        label={t('mail_fwd_addresses', { defaultValue: 'Adresses de transfert' })}
        description={t('mail_fwd_addresses_desc', {
          defaultValue: 'Réexpédiez une copie des messages entrants vers une autre adresse. Pour ne transférer que certains messages, créez un filtre.',
        })}
      >
        <div className="space-y-3">
          <div className="flex flex-wrap items-start gap-2">
            <div className="min-w-[240px]">
              <Input
                type="email"
                value={newAddr}
                onChange={e => { setNewAddr(e.target.value); setAddrError(null) }}
                onKeyDown={e => { if (e.key === 'Enter') { e.preventDefault(); addForward() } }}
                placeholder={t('mail_fwd_address_placeholder', { defaultValue: 'nom@exemple.com' })}
              />
              {addrError && <p className="text-xs text-danger mt-1">{addrError}</p>}
            </div>
            <Button variant="secondary" icon={<Plus size={16} />} onClick={addForward}>
              {t('mail_fwd_add_address', { defaultValue: 'Ajouter une adresse de transfert' })}
            </Button>
          </div>

          {prefs.forwardAddresses.length === 0 ? (
            <p className="text-sm text-text-tertiary">
              {t('mail_fwd_no_address', { defaultValue: 'Aucune adresse de transfert.' })}
            </p>
          ) : (
            <ul className="divide-y divide-[#e8eaed] border border-[#e8eaed] rounded-lg">
              {prefs.forwardAddresses.map(a => (
                <li key={a.id} className="flex items-center gap-3 px-3 py-2">
                  <Checkbox
                    checked={a.enabled}
                    onChange={v => toggleForward(a.id, v)}
                    label={a.email}
                  />
                  <span className="flex-1" />
                  <button
                    onClick={() => removeForward(a.id)}
                    className="p-1.5 rounded text-text-tertiary hover:text-danger hover:bg-danger/10"
                    title={t('mail_fwd_remove', { defaultValue: 'Supprimer' })}
                  >
                    <Trash2 size={14} />
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      </SettingsRow>

      <SettingsRow label={t('mail_fwd_local_copy', { defaultValue: 'Copie locale' })}>
        <RadioGroup
          value={prefs.forwardKeep ? 'keep' : 'drop'}
          onChange={v => set('forwardKeep', v === 'keep')}
          options={[
            { value: 'keep', label: t('mail_fwd_keep_copy', { defaultValue: 'Conserver la copie de Kubuno dans la boîte de réception' }) },
            { value: 'drop', label: t('mail_fwd_archive_copy', { defaultValue: 'Archiver la copie de Kubuno' }) },
          ]}
        />
      </SettingsRow>

      {/* ── POP download ───────────────────────────────────────────────── */}
      <Section title={t('mail_fwd_section_pop', { defaultValue: 'Téléchargement POP' })} />

      <SettingsRow
        label={t('mail_fwd_pop_status', { defaultValue: '1. État' })}
        description={t('mail_fwd_pop_status_desc', { defaultValue: 'Autoriser le relevé des messages via POP3.' })}
      >
        <RadioGroup
          value={popImap.popState}
          onChange={v => setPi('popState', v as PopImapSettings['popState'])}
          options={[
            { value: 'disabled', label: t('mail_fwd_pop_disabled', { defaultValue: 'POP désactivé' }) },
            { value: 'all',      label: t('mail_fwd_pop_all', { defaultValue: 'Activer POP pour tous les messages' }) },
            { value: 'from_now', label: t('mail_fwd_pop_from_now', { defaultValue: 'Activer POP pour les messages reçus à partir de maintenant' }) },
          ]}
        />
      </SettingsRow>

      <SettingsRow label={t('mail_fwd_pop_on_fetch', { defaultValue: '2. Lorsque les messages sont récupérés via POP' })}>
        <Dropdown
          width={320}
          value={popImap.popOnFetch}
          onChange={v => setPi('popOnFetch', v as PopImapSettings['popOnFetch'])}
          options={[
            { value: 'keep',      label: t('mail_fwd_pop_keep', { defaultValue: 'Conserver la copie de Kubuno dans la boîte de réception' }) },
            { value: 'mark_read', label: t('mail_fwd_pop_mark_read', { defaultValue: 'Marquer la copie de Kubuno comme lue' }) },
            { value: 'archive',   label: t('mail_fwd_pop_archive', { defaultValue: 'Archiver la copie de Kubuno' }) },
            { value: 'delete',    label: t('mail_fwd_pop_delete', { defaultValue: 'Supprimer la copie de Kubuno' }) },
          ]}
        />
      </SettingsRow>

      <SettingsRow label={t('mail_fwd_pop_configure', { defaultValue: '3. Configurez votre client' })}>
        <ConnectionInfo protocol="pop" />
      </SettingsRow>

      {/* ── IMAP access ────────────────────────────────────────────────── */}
      <Section title={t('mail_fwd_section_imap', { defaultValue: 'Accès IMAP' })} />

      <SettingsRow
        label={t('mail_fwd_imap_status', { defaultValue: 'État' })}
        description={t('mail_fwd_imap_status_desc', { defaultValue: 'Autoriser l’accès à cette boîte via IMAP.' })}
      >
        <RadioGroup
          value={popImap.imapEnabled ? 'on' : 'off'}
          onChange={v => setPi('imapEnabled', v === 'on')}
          options={[
            { value: 'on',  label: t('mail_fwd_imap_on', { defaultValue: 'Activer IMAP' }) },
            { value: 'off', label: t('mail_fwd_imap_off', { defaultValue: 'Désactiver IMAP' }) },
          ]}
        />
      </SettingsRow>

      <SettingsRow label={t('mail_fwd_imap_expunge', { defaultValue: 'Lorsque je marque un message comme supprimé dans IMAP' })}>
        <RadioGroup
          value={popImap.imapExpunge}
          onChange={v => setPi('imapExpunge', v as PopImapSettings['imapExpunge'])}
          options={[
            { value: 'auto', label: t('mail_fwd_imap_expunge_auto', { defaultValue: 'Effacer définitivement le message immédiatement' }) },
            { value: 'wait', label: t('mail_fwd_imap_expunge_client', { defaultValue: 'Attendre la mise à jour par le client (le client vide le dossier)' }) },
          ]}
        />
      </SettingsRow>

      <SettingsRow label={t('mail_fwd_imap_purge', { defaultValue: 'Lorsqu’un message est supprimé du dernier dossier IMAP visible' })}>
        <RadioGroup
          value={popImap.imapPurge}
          onChange={v => setPi('imapPurge', v as PopImapSettings['imapPurge'])}
          options={[
            { value: 'archive', label: t('mail_fwd_imap_purge_archive', { defaultValue: 'Archiver le message' }) },
            { value: 'trash',   label: t('mail_fwd_imap_purge_trash', { defaultValue: 'Déplacer le message vers la corbeille' }) },
            { value: 'delete',  label: t('mail_fwd_imap_purge_delete', { defaultValue: 'Supprimer le message définitivement' }) },
          ]}
        />
      </SettingsRow>

      <SettingsRow
        label={t('mail_fwd_imap_folder_limit', { defaultValue: 'Limites de taille des dossiers' })}
        description={t('mail_fwd_imap_folder_limit_desc', { defaultValue: 'Nombre maximal de messages exposés par dossier IMAP.' })}
      >
        <Dropdown
          width={280}
          value={popImap.imapFolderLimit}
          onChange={v => setPi('imapFolderLimit', v)}
          options={[
            { value: '0',     label: t('mail_fwd_imap_no_limit', { defaultValue: 'Ne pas limiter le nombre de messages' }) },
            { value: '1000',  label: t('mail_fwd_imap_limit_n', { defaultValue: 'Limiter à {{n}} messages', n: '1000' }) },
            { value: '2000',  label: t('mail_fwd_imap_limit_n', { defaultValue: 'Limiter à {{n}} messages', n: '2000' }) },
            { value: '5000',  label: t('mail_fwd_imap_limit_n', { defaultValue: 'Limiter à {{n}} messages', n: '5000' }) },
            { value: '10000', label: t('mail_fwd_imap_limit_n', { defaultValue: 'Limiter à {{n}} messages', n: '10000' }) },
          ]}
        />
      </SettingsRow>

      <SettingsRow label={t('mail_fwd_imap_configure', { defaultValue: 'Configurez votre client' })}>
        <ConnectionInfo protocol="imap" />
      </SettingsRow>

      {/* ── Save ───────────────────────────────────────────────────────── */}
      <div className="pt-6 flex items-center gap-3">
        <Button onClick={save}>
          {saved
            ? <><Check size={14} className="mr-1.5 inline" />{t('mail_settings_saved', { defaultValue: 'Enregistré' })}</>
            : t('mail_settings_save_changes', { defaultValue: 'Enregistrer les modifications' })
          }
        </Button>
        {saveError && <p className="text-sm text-danger">{saveError}</p>}
      </div>
    </div>
  )
}
