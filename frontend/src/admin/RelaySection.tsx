/**
 * Outbound relay (smarthost) — where an administrator points outgoing mail at a
 * single SMTP host instead of letting the queue reach each recipient's MX.
 *
 * ── Why this screen exists ───────────────────────────────────────────────────
 * By default the outbound worker delivers direct-to-MX, which is right for a
 * server with a public IP. It cannot work behind a residential line (port 25
 * blocked outbound) whose only route out is a trusted host — a VPS's Postfix
 * over a private tunnel. Turning this on makes every remote message go through
 * that host, which then reaches the world from a public IP with a good
 * reputation.
 *
 * ── The password is write-only ───────────────────────────────────────────────
 * The API never returns it — only whether one is set. So the field starts empty
 * with a "●●● défini" placeholder when a password exists; typing sets a new one,
 * leaving it blank keeps the stored one, and an explicit checkbox wipes it.
 */
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Server } from 'lucide-react'
import { Button, Callout, Checkbox, Input, NumberInput, Radio, Toggle } from '@ui'
import { mailAdminApi, type RelayConfig } from './api'
import { Section } from './chrome'

const RELAY_KEY = ['mail-admin-relay'] as const

type Security = 'none' | 'starttls' | 'tls'

interface FormState {
  enabled:  boolean
  host:     string
  port:     number
  security: Security
  username: string
  /** What the user typed. Empty = keep the stored password. */
  password: string
  /** Wipe the stored password on save. */
  clearPassword: boolean
}

function fromConfig(c: RelayConfig): FormState {
  return {
    enabled:  c.enabled,
    host:     c.host,
    port:     c.port || 25,
    security: (['none', 'starttls', 'tls'].includes(c.security) ? c.security : 'none') as Security,
    username: c.username,
    password: '',
    clearPassword: false,
  }
}

export default function RelaySection() {
  const { t } = useTranslation('mail')
  const qc = useQueryClient()

  const { data: config, isLoading } = useQuery({
    queryKey: RELAY_KEY,
    queryFn:  mailAdminApi.getRelay,
  })

  const [form, setForm] = useState<FormState | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [saved, setSaved] = useState(false)

  // Seed the form once the config arrives, and re-seed after a save returns the
  // fresh state — so `has_password` and the "●●● défini" placeholder stay honest.
  useEffect(() => {
    if (config) setForm(fromConfig(config))
  }, [config])

  const saveMut = useMutation({
    mutationFn: (f: FormState) => mailAdminApi.updateRelay({
      enabled:        f.enabled,
      host:           f.host.trim(),
      port:           f.port,
      security:       f.security,
      username:       f.username.trim(),
      // Only send a password when one was typed — an empty field keeps the
      // stored one; the checkbox is the only way to remove it.
      password:       f.password ? f.password : undefined,
      clear_password: f.clearPassword || undefined,
    }),
    onSuccess: (fresh) => {
      setError(null)
      setSaved(true)
      window.setTimeout(() => setSaved(false), 2500)
      qc.setQueryData(RELAY_KEY, fresh)
      setForm(fromConfig(fresh))
    },
    onError: (e: unknown) => setError(messageOf(e, t('relay_save_failed', {
      defaultValue: 'Enregistrement impossible',
    }))),
  })

  if (isLoading || !form) {
    return (
      <Section icon={<Server size={16} />} title={t('relay_title', { defaultValue: 'Relais sortant' })}>
        <div className="py-6 text-center text-sm text-text-secondary">
          {t('loading', { defaultValue: 'Chargement…' })}
        </div>
      </Section>
    )
  }

  const set = <K extends keyof FormState>(key: K, value: FormState[K]) =>
    setForm(prev => (prev ? { ...prev, [key]: value } : prev))

  // The two client-side mirrors of the server's rules — shown inline so a save
  // that would be refused is disabled before it is attempted.
  const hostMissing = form.enabled && form.host.trim() === ''
  const clearAuthNoTls = form.security === 'none' && form.username.trim() !== '' && !form.clearPassword
  const invalid = hostMissing || clearAuthNoTls

  const securities: { value: Security; label: string; description: string }[] = [
    {
      value: 'none',
      label: t('relay_sec_none', { defaultValue: 'Aucun chiffrement' }),
      description: t('relay_sec_none_desc', {
        defaultValue: 'En clair. À réserver à un lien privé de confiance (le tunnel vers votre VPS).',
      }),
    },
    {
      value: 'starttls',
      label: 'STARTTLS',
      description: t('relay_sec_starttls_desc', {
        defaultValue: 'Connexion en clair puis passage en TLS. Le port 587 (submission) typiquement.',
      }),
    },
    {
      value: 'tls',
      label: t('relay_sec_tls', { defaultValue: 'TLS implicite' }),
      description: t('relay_sec_tls_desc', {
        defaultValue: 'TLS dès la connexion. Le port 465 typiquement.',
      }),
    },
  ]

  return (
    <Section
      icon={<Server size={16} />}
      title={t('relay_title', { defaultValue: 'Relais sortant' })}
      description={t('relay_intro', {
        defaultValue:
          'Par défaut, chaque message part directement vers le serveur (MX) de son destinataire. Activez un relais si votre ligne bloque le port 25 sortant — le cas d’une connexion résidentielle : tout le courrier distant est alors remis à un unique serveur SMTP, qui le livre à votre place.',
      })}
    >
      <Callout variant="info" title={t('relay_help_title', { defaultValue: 'Quand configurer un relais ?' })}>
        {t('relay_help_body', {
          defaultValue:
            'Sur une ligne résidentielle, le port 25 sortant est presque toujours bloqué : la livraison directe au MX est impossible, et un relais (le Postfix de votre VPS) est le seul chemin vers internet. Sur un tunnel privé de confiance, « Aucun chiffrement » sans authentification suffit — l’hôte et le port sont tout ce qu’il faut. Pour un relais public, utilisez STARTTLS ou TLS avec un identifiant : sans chiffrement, le mot de passe circulerait en clair.',
        })}
      </Callout>

      <div className="mt-4 space-y-5">
        <Toggle
          checked={form.enabled}
          onChange={e => set('enabled', e.target.checked)}
          label={t('relay_enabled', { defaultValue: 'Router le courrier sortant via un relais' })}
          description={t('relay_enabled_desc', {
            defaultValue: 'Désactivé : livraison directe au MX de chaque destinataire (comportement par défaut).',
          })}
        />

        <div className="grid gap-4 sm:grid-cols-[2fr_1fr]">
          <div>
            <label className="mb-1 block text-xs text-text-secondary" htmlFor="relay-host">
              {t('relay_host', { defaultValue: 'Hôte du relais' })}
            </label>
            <Input
              id="relay-host"
              value={form.host}
              onChange={e => set('host', e.target.value)}
              placeholder="15.100.1.1"
            />
            {hostMissing && (
              <p className="mt-1 text-xs text-danger">
                {t('relay_host_required', { defaultValue: 'Un relais activé exige un hôte.' })}
              </p>
            )}
          </div>
          <div>
            <label className="mb-1 block text-xs text-text-secondary" htmlFor="relay-port">
              {t('relay_port', { defaultValue: 'Port' })}
            </label>
            <NumberInput
              id="relay-port"
              value={form.port}
              onChange={v => set('port', v)}
              min={1}
              max={65535}
            />
          </div>
        </div>

        <div>
          <span className="mb-2 block text-xs text-text-secondary">
            {t('relay_security', { defaultValue: 'Chiffrement de la connexion au relais' })}
          </span>
          <div className="space-y-2">
            {securities.map(s => (
              <Radio
                key={s.value}
                checked={form.security === s.value}
                onChange={() => set('security', s.value)}
                label={s.label}
                description={s.description}
              />
            ))}
          </div>
        </div>

        <div className="grid gap-4 sm:grid-cols-2">
          <div>
            <label className="mb-1 block text-xs text-text-secondary" htmlFor="relay-username">
              {t('relay_username', { defaultValue: 'Nom d’utilisateur (facultatif)' })}
            </label>
            <Input
              id="relay-username"
              value={form.username}
              onChange={e => set('username', e.target.value)}
              placeholder={t('relay_username_ph', { defaultValue: 'vide = relais sans authentification' })}
              autoComplete="off"
            />
          </div>
          <div>
            <label className="mb-1 block text-xs text-text-secondary" htmlFor="relay-password">
              {t('relay_password', { defaultValue: 'Mot de passe' })}
            </label>
            <Input
              id="relay-password"
              type="password"
              value={form.password}
              onChange={e => set('password', e.target.value)}
              disabled={form.clearPassword}
              placeholder={config?.has_password && !form.clearPassword
                ? t('relay_password_set', { defaultValue: '●●● défini — laisser vide pour conserver' })
                : t('relay_password_ph', { defaultValue: 'Saisir un mot de passe' })}
              autoComplete="new-password"
            />
            {config?.has_password && (
              <div className="mt-2">
                <Checkbox
                  checked={form.clearPassword}
                  onChange={(checked: boolean) => set('clearPassword', checked)}
                  label={t('relay_password_clear', { defaultValue: 'Supprimer le mot de passe enregistré' })}
                />
              </div>
            )}
          </div>
        </div>

        {clearAuthNoTls && (
          <Callout variant="warning">
            {t('relay_cleartext_password', {
              defaultValue:
                'Un mot de passe ne doit pas transiter en clair — utilisez STARTTLS ou TLS, ou un relais sans authentification sur réseau de confiance.',
            })}
          </Callout>
        )}

        {error && (
          <p className="rounded border border-danger bg-danger-light p-2 text-sm text-danger">{error}</p>
        )}

        <div className="flex items-center gap-3">
          <Button
            onClick={() => { if (form) saveMut.mutate(form) }}
            disabled={invalid || saveMut.isPending}
            loading={saveMut.isPending}
          >
            {t('save', { defaultValue: 'Enregistrer' })}
          </Button>
          {saved && (
            <span className="text-sm text-success">
              {t('relay_saved', { defaultValue: 'Enregistré' })}
            </span>
          )}
        </div>
      </div>
    </Section>
  )
}

/** The server's own validation sentence, or the fallback — same normalisation
 *  quirk as the DKIM section (the SDK's axios client flattens rejections). */
function messageOf(error: unknown, fallback: string): string {
  const nested = (error as { response?: { data?: { message?: string } } })?.response?.data?.message
  if (typeof nested === 'string' && nested.length > 0) return nested
  const flat = (error as { message?: unknown })?.message
  return typeof flat === 'string' && flat.length > 0 ? flat : fallback
}
