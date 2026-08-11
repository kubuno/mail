import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { KeyRound, Trash2, Loader2, Plus, Upload, Copy, ShieldCheck, Users } from 'lucide-react'
import { useConfirm } from '@kubuno/sdk'
import { Button, Input, ConfirmDialog } from '@ui'
import { mailApi } from '../api'

/** Group a hex fingerprint into 4-char blocks, the customary display form. */
function fmtFingerprint(fp: string): string {
  return (fp.match(/.{1,4}/g) ?? [fp]).join(' ')
}

export function EncryptionTab() {
  const { t } = useTranslation('mail')
  const qc = useQueryClient()
  const { confirm, confirmState, handleConfirm, handleCancel } = useConfirm()

  const { data: status, isLoading: statusLoading } = useQuery({
    queryKey: ['mail-pgp-status'], queryFn: mailApi.pgpStatus,
  })
  const enabled = !!status?.enabled

  const { data: keysData } = useQuery({
    queryKey: ['mail-pgp-keys'], queryFn: mailApi.pgpListKeys, enabled,
  })
  const { data: contactsData } = useQuery({
    queryKey: ['mail-pgp-contacts'], queryFn: mailApi.pgpListContacts, enabled,
  })
  const keys = keysData?.keys ?? []
  const contacts = contactsData?.contacts ?? []

  // ── Forms ───────────────────────────────────────────────────────────────────
  const [genName, setGenName]   = useState('')
  const [genEmail, setGenEmail] = useState('')
  const [importArmored, setImportArmored] = useState('')
  const [importPass, setImportPass]       = useState('')
  const [contactEmail, setContactEmail]   = useState('')
  const [contactArmored, setContactArmored] = useState('')

  const invalidateKeys     = () => qc.invalidateQueries({ queryKey: ['mail-pgp-keys'] })
  const invalidateContacts = () => qc.invalidateQueries({ queryKey: ['mail-pgp-contacts'] })

  const genMut = useMutation({
    mutationFn: () => mailApi.pgpGenerateKey({ name: genName.trim() || undefined, email: genEmail.trim() }),
    onSuccess: () => { setGenName(''); setGenEmail(''); invalidateKeys() },
  })
  const importMut = useMutation({
    mutationFn: () => mailApi.pgpImportKey({ secret_armored: importArmored, passphrase: importPass || undefined }),
    onSuccess: () => { setImportArmored(''); setImportPass(''); invalidateKeys() },
  })
  const deleteKeyMut = useMutation({
    mutationFn: (id: string) => mailApi.pgpDeleteKey(id),
    onSuccess: invalidateKeys,
  })
  const addContactMut = useMutation({
    mutationFn: () => mailApi.pgpAddContact({ email: contactEmail.trim() || undefined, public_armored: contactArmored }),
    onSuccess: () => { setContactEmail(''); setContactArmored(''); invalidateContacts() },
  })
  const deleteContactMut = useMutation({
    mutationFn: (id: string) => mailApi.pgpDeleteContact(id),
    onSuccess: invalidateContacts,
  })

  const copyPublic = (armored: string) => { void navigator.clipboard?.writeText(armored) }

  if (statusLoading) {
    return <div className="flex justify-center py-8"><Loader2 size={20} className="animate-spin text-text-tertiary" /></div>
  }

  // Disabled instance-wide: the tab explains itself rather than offering dead controls.
  if (!enabled) {
    return (
      <div className="text-center py-14">
        <KeyRound size={32} className="opacity-30 mx-auto mb-3 text-text-tertiary" />
        <p className="text-sm text-text-primary font-medium">{t('mail_pgp_disabled_title', { defaultValue: 'Chiffrement OpenPGP désactivé' })}</p>
        <p className="text-xs text-text-tertiary mt-1 max-w-md mx-auto">
          {t('mail_pgp_disabled_hint', { defaultValue: "L'administrateur de cette instance n'a pas activé OpenPGP. Contactez-le pour l'activer dans la console d'administration." })}
        </p>
      </div>
    )
  }

  return (
    <div className="space-y-8">
      {/* ── Your identities ─────────────────────────────────────────────────── */}
      <section>
        <div className="flex items-center gap-2 mb-3">
          <ShieldCheck size={16} className="text-text-secondary" />
          <h3 className="text-sm font-medium text-text-primary">{t('mail_pgp_your_keys', { defaultValue: 'Vos clés' })}</h3>
        </div>

        {keys.length === 0 ? (
          <p className="text-sm text-text-tertiary py-2">{t('mail_pgp_no_keys', { defaultValue: "Vous n'avez pas encore de clé OpenPGP." })}</p>
        ) : (
          <ul className="mb-4">
            {keys.map(k => (
              <li key={k.id} className="flex items-center justify-between py-2.5 border-b border-[#e8eaed] last:border-0">
                <div className="min-w-0">
                  <div className="flex items-center gap-2">
                    <span className="text-sm text-text-primary truncate">{k.email || t('mail_pgp_no_address', { defaultValue: '(sans adresse)' })}</span>
                    {k.is_default && (
                      <span className="text-xs text-primary bg-primary/10 px-1.5 py-0.5 rounded">{t('mail_pgp_default', { defaultValue: 'Par défaut' })}</span>
                    )}
                  </div>
                  <div className="text-xs text-text-tertiary font-mono mt-0.5">{fmtFingerprint(k.fingerprint)}</div>
                </div>
                <div className="flex items-center gap-1 flex-shrink-0">
                  <button onClick={() => copyPublic(k.public_key)}
                    className="p-1.5 rounded text-text-tertiary hover:text-primary hover:bg-primary/10"
                    title={t('mail_pgp_copy_public', { defaultValue: 'Copier la clé publique' })}>
                    <Copy size={15} />
                  </button>
                  <button
                    onClick={async () => {
                      if (await confirm({
                        title: t('mail_pgp_delete_key_title', { defaultValue: 'Supprimer cette clé ?' }),
                        message: t('mail_pgp_delete_key_msg', { defaultValue: 'La clé privée sera définitivement supprimée de cette instance. Les messages déjà chiffrés avec elle deviendront illisibles.' }),
                        confirmLabel: t('delete', { defaultValue: 'Supprimer' }),
                        variant: 'danger',
                      })) deleteKeyMut.mutate(k.id)
                    }}
                    className="p-1.5 rounded text-text-tertiary hover:text-danger hover:bg-danger/10"
                    title={t('delete', { defaultValue: 'Supprimer' })}>
                    <Trash2 size={15} />
                  </button>
                </div>
              </li>
            ))}
          </ul>
        )}

        <div className="grid gap-4 md:grid-cols-2">
          {/* Generate */}
          <div className="p-4 border border-border rounded-xl bg-surface-1">
            <p className="text-sm font-medium text-text-primary mb-2">{t('mail_pgp_generate', { defaultValue: 'Générer une nouvelle clé' })}</p>
            <div className="space-y-2">
              <Input value={genName} onChange={e => setGenName(e.target.value)} placeholder={t('mail_pgp_name', { defaultValue: 'Nom (facultatif)' })} />
              <Input value={genEmail} onChange={e => setGenEmail(e.target.value)} placeholder={t('mail_pgp_email', { defaultValue: 'Adresse e-mail' })} type="email" />
              {genMut.isError && <p className="text-xs text-danger">{t('mail_pgp_gen_failed', { defaultValue: 'La génération a échoué.' })}</p>}
              <Button size="sm" onClick={() => genMut.mutate()}
                disabled={!genEmail.includes('@') || genMut.isPending}
                icon={genMut.isPending ? <Loader2 size={14} className="animate-spin" /> : <Plus size={14} />}>
                {t('mail_pgp_generate_btn', { defaultValue: 'Générer' })}
              </Button>
            </div>
          </div>

          {/* Import */}
          <div className="p-4 border border-border rounded-xl bg-surface-1">
            <p className="text-sm font-medium text-text-primary mb-2">{t('mail_pgp_import', { defaultValue: 'Importer une clé existante' })}</p>
            <div className="space-y-2">
              <textarea value={importArmored} onChange={e => setImportArmored(e.target.value)}
                placeholder="-----BEGIN PGP PRIVATE KEY BLOCK-----"
                className="w-full h-24 px-2 py-1.5 text-xs font-mono rounded border border-border bg-surface-0 focus:outline-none focus:border-primary resize-y" />
              <Input value={importPass} onChange={e => setImportPass(e.target.value)} type="password" placeholder={t('mail_pgp_passphrase', { defaultValue: 'Passphrase (si protégée)' })} />
              {importMut.isError && <p className="text-xs text-danger">{t('mail_pgp_import_failed', { defaultValue: 'Import impossible : clé ou passphrase invalide.' })}</p>}
              <Button size="sm" onClick={() => importMut.mutate()}
                disabled={!importArmored.includes('PGP PRIVATE KEY') || importMut.isPending}
                icon={importMut.isPending ? <Loader2 size={14} className="animate-spin" /> : <Upload size={14} />}>
                {t('mail_pgp_import_btn', { defaultValue: 'Importer' })}
              </Button>
            </div>
          </div>
        </div>
      </section>

      {/* ── Correspondents' keys ────────────────────────────────────────────── */}
      <section>
        <div className="flex items-center gap-2 mb-3">
          <Users size={16} className="text-text-secondary" />
          <h3 className="text-sm font-medium text-text-primary">{t('mail_pgp_contacts', { defaultValue: 'Clés publiques de vos contacts' })}</h3>
        </div>

        {contacts.length === 0 ? (
          <p className="text-sm text-text-tertiary py-2">{t('mail_pgp_no_contacts', { defaultValue: 'Aucune clé publique enregistrée.' })}</p>
        ) : (
          <ul className="mb-4">
            {contacts.map(c => (
              <li key={c.id} className="flex items-center justify-between py-2.5 border-b border-[#e8eaed] last:border-0">
                <div className="min-w-0">
                  <span className="text-sm text-text-primary truncate">{c.email}</span>
                  <div className="text-xs text-text-tertiary font-mono mt-0.5">{fmtFingerprint(c.fingerprint)}</div>
                </div>
                <button
                  onClick={async () => {
                    if (await confirm({
                      title: t('mail_pgp_delete_contact_title', { defaultValue: 'Supprimer cette clé publique ?' }),
                      message: t('mail_pgp_delete_contact_msg', { defaultValue: 'Vous ne pourrez plus chiffrer de message pour ce contact tant que vous n’aurez pas réimporté sa clé.' }),
                      confirmLabel: t('delete', { defaultValue: 'Supprimer' }),
                      variant: 'danger',
                    })) deleteContactMut.mutate(c.id)
                  }}
                  className="p-1.5 rounded text-text-tertiary hover:text-danger hover:bg-danger/10 flex-shrink-0"
                  title={t('delete', { defaultValue: 'Supprimer' })}>
                  <Trash2 size={15} />
                </button>
              </li>
            ))}
          </ul>
        )}

        <div className="p-4 border border-border rounded-xl bg-surface-1">
          <p className="text-sm font-medium text-text-primary mb-2">{t('mail_pgp_add_contact', { defaultValue: 'Ajouter la clé publique d’un contact' })}</p>
          <div className="space-y-2">
            <Input value={contactEmail} onChange={e => setContactEmail(e.target.value)} placeholder={t('mail_pgp_contact_email', { defaultValue: "Adresse du contact (facultatif si la clé la contient)" })} type="email" />
            <textarea value={contactArmored} onChange={e => setContactArmored(e.target.value)}
              placeholder="-----BEGIN PGP PUBLIC KEY BLOCK-----"
              className="w-full h-24 px-2 py-1.5 text-xs font-mono rounded border border-border bg-surface-0 focus:outline-none focus:border-primary resize-y" />
            {addContactMut.isError && <p className="text-xs text-danger">{t('mail_pgp_add_contact_failed', { defaultValue: 'Clé publique invalide ou adresse manquante.' })}</p>}
            <Button size="sm" onClick={() => addContactMut.mutate()}
              disabled={!contactArmored.includes('PGP PUBLIC KEY') || addContactMut.isPending}
              icon={addContactMut.isPending ? <Loader2 size={14} className="animate-spin" /> : <Plus size={14} />}>
              {t('mail_pgp_add_contact_btn', { defaultValue: 'Ajouter' })}
            </Button>
          </div>
        </div>
      </section>

      {confirmState && (
        <ConfirmDialog {...confirmState} onConfirm={handleConfirm} onCancel={handleCancel} />
      )}
    </div>
  )
}
