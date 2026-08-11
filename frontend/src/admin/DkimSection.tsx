/**
 * DKIM signing keys — the section an administrator opens to publish a DNS
 * record, and comes back to a year later to rotate it.
 *
 * ── What this screen is really for ───────────────────────────────────────────
 * Generating a key is one click and nobody needs help with it. The hard part is
 * the record: `<selector>._domainkey.<domain> IN TXT "v=DKIM1; k=rsa; p=…"`,
 * some four hundred characters that must be pasted into a zone file EXACTLY.
 * So the record is shown whole — never truncated, never behind a "show" toggle
 * — with a copy button, for every key, active or not.
 *
 * ── Rotation, and why the selector is a field ────────────────────────────────
 * A signing key is replaced, not edited: DNS propagates over hours and
 * receivers cache, so overwriting the record under a fixed selector leaves a
 * window where in-flight signatures verify against a key that no longer exists.
 * The sequence is: create under a NEW selector (the key arrives inactive),
 * publish, wait, activate, delete the old one. Each of those four steps is one
 * control here, in that order.
 */
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { KeyRound, Trash2, ShieldCheck, CircleDot } from 'lucide-react'
import { Button, ConfirmDialog, Dropdown, Input, Spinner } from '@ui'
import { useConfirm } from '@kubuno/sdk'
import { mailAdminApi, type DkimKey } from './api'
import { CopyButton, RecordBlock, Section } from './chrome'

const DKIM_KEYS = ['mail-admin-dkim'] as const

export default function DkimSection() {
  const { t } = useTranslation('mail')
  const qc = useQueryClient()
  const { confirm, confirmState, handleConfirm, handleCancel } = useConfirm()

  const [domain,    setDomain]    = useState('')
  const [selector,  setSelector]  = useState('')
  const [algorithm, setAlgorithm] = useState('rsa-sha256')
  const [error,     setError]     = useState<string | null>(null)

  const { data: keys = [], isLoading } = useQuery({
    queryKey: DKIM_KEYS,
    queryFn:  mailAdminApi.listDkimKeys,
  })

  const invalidate = () => {
    void qc.invalidateQueries({ queryKey: DKIM_KEYS })
    // The diagnostic reads the same keys against DNS: leaving it stale would
    // show a light for a key that no longer exists.
    void qc.invalidateQueries({ queryKey: ['mail-admin-diagnostics'] })
  }

  const createMut = useMutation({
    mutationFn: () => mailAdminApi.createDkimKey({
      domain,
      selector:  selector.trim() || undefined,
      algorithm,
    }),
    onSuccess: () => {
      setDomain('')
      setSelector('')
      setError(null)
      invalidate()
    },
    onError: (e: unknown) => setError(messageOf(e, t('dkim_create_failed', {
      defaultValue: 'Génération impossible',
    }))),
  })

  const activateMut = useMutation({
    mutationFn: (id: string) => mailAdminApi.activateDkimKey(id),
    onSuccess:  invalidate,
    onError:    () => setError(t('dkim_activate_failed', { defaultValue: 'Activation impossible' })),
  })

  const deleteMut = useMutation({
    mutationFn: (id: string) => mailAdminApi.deleteDkimKey(id),
    onSuccess:  invalidate,
    onError:    () => setError(t('dkim_delete_failed', { defaultValue: 'Suppression impossible' })),
  })

  const askDelete = async (key: DkimKey) => {
    const ok = await confirm({
      title:   t('dkim_delete_title', { defaultValue: 'Supprimer cette clé DKIM ?' }),
      message: key.is_active
        ? t('dkim_delete_active_msg', {
          defaultValue:
            'C’est la clé qui signe le courrier de {{domain}}. Sans elle, les messages partent non signés et échouent DMARC chez les grands fournisseurs. Supprimez plutôt l’ancienne clé après avoir activé la nouvelle.',
          domain: key.domain,
        })
        : t('dkim_delete_msg', {
          defaultValue:
            'La clé privée est détruite : elle ne peut pas être régénérée à l’identique. Retirez ensuite l’enregistrement {{name}} de votre zone DNS.',
          name: key.dns_name,
        }),
      confirmLabel: t('delete', { defaultValue: 'Supprimer' }),
      variant: 'danger',
    })
    if (ok) deleteMut.mutate(key.id)
  }

  const askActivate = async (key: DkimKey) => {
    const ok = await confirm({
      title:   t('dkim_activate_title', { defaultValue: 'Signer avec cette clé ?' }),
      message: t('dkim_activate_msg', {
        defaultValue:
          'Le courrier de {{domain}} sera signé avec le sélecteur « {{selector}} ». Ne le faites qu’une fois l’enregistrement publié ET propagé : sinon les signatures émises seront invérifiables.',
        domain: key.domain, selector: key.selector,
      }),
      confirmLabel: t('dkim_activate_ok', { defaultValue: 'Activer' }),
    })
    if (ok) activateMut.mutate(key.id)
  }

  const algorithms = [
    { value: 'rsa-sha256',     label: 'RSA 2048' },
    { value: 'ed25519-sha256', label: 'Ed25519' },
  ]

  return (
    <Section
      icon={<KeyRound size={16} />}
      title={t('dkim_title', { defaultValue: 'Clés de signature DKIM' })}
      description={t('dkim_intro', {
        defaultValue:
          'Chaque domaine émetteur a une clé, publiée dans le DNS sous un sélecteur. Sans enregistrement publié, la signature est invérifiable et le courrier échoue DMARC. Pour renouveler une clé sans coupure : créez-en une sous un nouveau sélecteur, publiez-la, attendez la propagation, activez-la, puis supprimez l’ancienne.',
      })}
    >
      {/* Création */}
      <div className="mb-4 flex flex-wrap items-end gap-2">
        <div className="min-w-[220px]">
          <label className="mb-1 block text-xs text-text-secondary" htmlFor="dkim-domain">
            {t('dkim_domain', { defaultValue: 'Domaine' })}
          </label>
          <Input
            id="dkim-domain"
            value={domain}
            onChange={e => setDomain(e.target.value)}
            placeholder="example.com"
          />
        </div>
        <div className="min-w-[180px]">
          <label className="mb-1 block text-xs text-text-secondary" htmlFor="dkim-selector">
            {t('dkim_selector', { defaultValue: 'Sélecteur' })}
          </label>
          <Input
            id="dkim-selector"
            value={selector}
            onChange={e => setSelector(e.target.value)}
            placeholder="kubuno"
          />
        </div>
        <div className="min-w-[160px]">
          <label className="mb-1 block text-xs text-text-secondary">
            {t('dkim_algorithm', { defaultValue: 'Algorithme' })}
          </label>
          <Dropdown options={algorithms} value={algorithm} onChange={setAlgorithm} />
        </div>
        <Button
          onClick={() => createMut.mutate()}
          disabled={!domain.trim() || createMut.isPending}
          loading={createMut.isPending}
        >
          {t('dkim_create', { defaultValue: 'Générer une clé' })}
        </Button>
      </div>

      <p className="mb-4 text-xs text-text-tertiary">
        {t('dkim_selector_hint', {
          defaultValue:
            'Le sélecteur devient une étiquette DNS (lettres, chiffres, tirets). Laissé vide, il vaut « kubuno ». Une date — 2026a — rend la prochaine rotation évidente. Ed25519 donne un enregistrement court, mais certains vérificateurs anciens ne le connaissent pas : RSA 2048 reste le choix sûr.',
        })}
      </p>

      {error && (
        <p className="mb-3 rounded border border-danger bg-danger-light p-2 text-sm text-danger">{error}</p>
      )}

      {/* Liste */}
      {isLoading ? (
        <div className="flex justify-center py-6"><Spinner /></div>
      ) : keys.length === 0 ? (
        <p className="rounded border border-border bg-surface-1 p-3 text-sm text-text-secondary">
          {t('dkim_empty', {
            defaultValue:
              'Aucune clé. Tant qu’un domaine n’en a pas, son courrier part non signé — et non signé, il est rejeté par Gmail, Yahoo et Microsoft au-delà de quelques milliers de messages par jour.',
          })}
        </p>
      ) : (
        <ul className="space-y-3">
          {keys.map(key => (
            <li key={key.id} className="rounded-lg border border-border p-3">
              <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
                <div className="flex min-w-0 flex-wrap items-center gap-2">
                  <span className="text-sm font-bold text-text-primary">{key.domain}</span>
                  <span className="text-xs text-text-secondary">
                    {t('dkim_row_meta', {
                      defaultValue: 'sélecteur {{selector}} · {{algo}} · créée le {{date}}',
                      selector: key.selector,
                      algo:     key.algorithm === 'ed25519-sha256' ? 'Ed25519' : 'RSA 2048',
                      date:     new Date(key.created_at).toLocaleDateString(),
                    })}
                  </span>
                  {key.is_active ? (
                    <span className="inline-flex items-center gap-1 rounded-full bg-success-light px-2 py-0.5 text-success"
                      style={{ fontSize: 'var(--kb-text-micro, 11px)' }}>
                      <ShieldCheck size={11} />
                      {t('dkim_active', { defaultValue: 'Signe ce domaine' })}
                    </span>
                  ) : (
                    <span className="inline-flex items-center gap-1 rounded-full bg-surface-2 px-2 py-0.5 text-text-secondary"
                      style={{ fontSize: 'var(--kb-text-micro, 11px)' }}>
                      <CircleDot size={11} />
                      {t('dkim_staged', { defaultValue: 'En attente' })}
                    </span>
                  )}
                </div>
                <div className="flex shrink-0 items-center gap-1">
                  {!key.is_active && (
                    <Button variant="secondary" size="sm" onClick={() => void askActivate(key)}>
                      {t('dkim_activate', { defaultValue: 'Activer' })}
                    </Button>
                  )}
                  <Button variant="ghost" size="sm" onClick={() => void askDelete(key)}
                    aria-label={t('delete', { defaultValue: 'Supprimer' })}>
                    <Trash2 size={14} />
                  </Button>
                </div>
              </div>

              {/* THE thing an administrator came for. Whole, copyable, both as
                  a zone-file line and as the value alone — the two shapes DNS
                  interfaces ask for. */}
              <RecordBlock
                name={`${key.dns_name}.  IN  TXT`}
                value={`"${key.dns_value}"`}
                onCopyAll={`${key.dns_name}. IN TXT "${key.dns_value}"`}
              />
              <div className="mt-1 flex items-center gap-2 text-xs text-text-tertiary">
                {t('dkim_value_only', { defaultValue: 'Valeur seule (formulaires DNS)' })}
                <CopyButton value={key.dns_value} />
              </div>
            </li>
          ))}
        </ul>
      )}

      {confirmState && (
        <ConfirmDialog {...confirmState} onConfirm={handleConfirm} onCancel={handleCancel} />
      )}
    </Section>
  )
}

/** Surfaces the server's own validation message ("Sélecteur invalide : …")
 *  rather than a generic failure — it names exactly what to fix. */
/**
 * The sentence the server wrote, or the fallback.
 *
 * ⚠️ Reading only `error.response.data.message` never found anything: the SDK's
 * axios client NORMALISES its rejections into a flat `{ message, code }` and
 * there is no `response` to walk. So every failure here showed the generic
 * fallback, hiding answers like "l'adresse est déjà un alias" that were written
 * precisely to be read. Both shapes are accepted so this keeps working whichever
 * client the section is mounted under.
 */
function messageOf(error: unknown, fallback: string): string {
  const nested = (error as { response?: { data?: { message?: string } } })?.response?.data?.message
  if (typeof nested === 'string' && nested.length > 0) return nested
  const flat = (error as { message?: unknown })?.message
  return typeof flat === 'string' && flat.length > 0 ? flat : fallback
}
