/**
 * The pieces the four address tabs share: the address field, the quota field,
 * the badges that tell an inert row from a working one, and the one-shot
 * password panel.
 *
 * They live here rather than in each tab because the four tabs are read as ONE
 * panel — a domain that is "not served" must look identical whether it is seen
 * from a mailbox, an alias or a list, otherwise the operator has to learn three
 * vocabularies for one fact.
 */
import { useEffect, useState, type ReactNode } from 'react'
import { useTranslation } from 'react-i18next'
import { AlertTriangle, Check, Copy, HelpCircle, KeyRound } from 'lucide-react'
import { Badge, Button, Callout, Combobox, Dropdown, Input, NumberInput } from '@ui'
import { formatSize } from '@kubuno/sdk'
import type { DirectoryUser } from './api'

// ── Small helpers ────────────────────────────────────────────────────────────

/** Debounces a value — the search fields hit paginated routes on every key. */
export function useDebounced<T>(value: T, delay = 300): T {
  const [held, setHeld] = useState(value)
  useEffect(() => {
    const id = window.setTimeout(() => setHeld(value), delay)
    return () => window.clearTimeout(id)
  }, [value, delay])
  return held
}

/** A quota as a sentence. `0` is unlimited, and says so rather than "0 o". */
export function quotaLabel(bytes: number, unlimited: string): string {
  return bytes > 0 ? formatSize(bytes) : unlimited
}

/** One destination per line — how the operator types a set of addresses. */
export function linesToAddresses(text: string): string[] {
  return text
    .split(/[\n,;]+/)
    .map(s => s.trim())
    .filter(s => s.length > 0)
}

export function addressesToLines(list: string[]): string {
  return list.join('\n')
}

// ── Badges ───────────────────────────────────────────────────────────────────

/**
 * What the row's domain is worth. THREE states, never two: `false` says nothing
 * reaches this address any more, `null` says the setting could not be read.
 * Collapsing them would make an unreachable core look like a broken domain.
 */
export function DomainBadge({ served }: { served: boolean | null }) {
  const { t } = useTranslation('mail')
  if (served === true) return null
  if (served === false) {
    return (
      <Badge variant="danger" size="sm">
        {t('addr_domain_unserved', { defaultValue: 'Domaine non servi' })}
      </Badge>
    )
  }
  return (
    <Badge variant="neutral" size="sm">
      {t('addr_domain_unknown', { defaultValue: 'Domaine non vérifié' })}
    </Badge>
  )
}

export function ActiveBadge({ active }: { active: boolean }) {
  const { t } = useTranslation('mail')
  return active
    ? <Badge variant="success" size="sm">{t('addr_active', { defaultValue: 'Actif' })}</Badge>
    : <Badge variant="warning" size="sm">{t('addr_suspended', { defaultValue: 'Suspendu' })}</Badge>
}

// ── Errors ───────────────────────────────────────────────────────────────────

/** The server's sentence, kept whole and kept on screen. */
export function ErrorNote({ message, onDismiss }: { message: string | null; onDismiss?: () => void }) {
  const { t } = useTranslation('mail')
  if (!message) return null
  return (
    <Callout
      variant="danger"
      className="mb-3"
      title={t('addr_refused', { defaultValue: 'Refusé par le serveur' })}
      dismissible={!!onDismiss}
      onDismiss={onDismiss}
    >
      {message}
    </Callout>
  )
}

// ── Fields ───────────────────────────────────────────────────────────────────

/** Labelled block, so every form in the panel aligns the same way. */
export function Field({ label, hint, htmlFor, children }: {
  label:    string
  hint?:    ReactNode
  htmlFor?: string
  children: ReactNode
}) {
  return (
    <div className="min-w-0">
      <label className="mb-1 block text-xs text-text-secondary" htmlFor={htmlFor}>{label}</label>
      {children}
      {hint && (
        <p className="mt-1 leading-relaxed text-text-tertiary"
          style={{ fontSize: 'var(--kb-text-micro, 11px)' }}>{hint}</p>
      )}
    </div>
  )
}

/**
 * An address, as local part + a domain PICKED from the served ones.
 *
 * The domain is never typed: an address in a domain nothing routes to us is a
 * row that receives nothing for weeks before anybody notices, and the server
 * refuses it anyway. Offering the list is the only way to make that
 * impossible-by-construction rather than caught-after-the-fact.
 *
 * `allowCatchAll` lets the local part be empty, which is how `@domaine` — the
 * catch-all — is written. Only the alias form allows it.
 */
export function AddressField({
  local, domain, domains, onLocal, onDomain, allowCatchAll = false, disabled = false, id,
}: {
  local:   string
  domain:  string
  domains: string[]
  onLocal:  (v: string) => void
  onDomain: (v: string) => void
  allowCatchAll?: boolean
  disabled?: boolean
  id?: string
}) {
  const { t } = useTranslation('mail')
  const options = domains.map(d => ({ value: d, label: `@${d}` }))

  return (
    <div className="flex flex-wrap items-start gap-2">
      <div className="min-w-[180px] flex-1">
        <Input
          id={id}
          value={local}
          disabled={disabled}
          onChange={e => onLocal(e.target.value.trim())}
          placeholder={allowCatchAll
            ? t('addr_local_or_empty', { defaultValue: 'contact (vide = attrape-tout)' })
            : t('addr_local', { defaultValue: 'contact' })}
          aria-label={t('addr_local_part', { defaultValue: 'Partie locale' })}
        />
      </div>
      <div className="min-w-[200px]">
        {domains.length > 0 ? (
          <Dropdown
            value={domain}
            onChange={onDomain}
            options={options}
            disabled={disabled}
            focusable
            height={36}
            width="100%"
            placeholder={t('addr_pick_domain', { defaultValue: 'Domaine…' })}
          />
        ) : (
          <p className="text-xs text-danger">
            {t('addr_no_served_domain', {
              defaultValue: 'Aucun domaine servi : renseignez « server_domains » dans Services et ports.',
            })}
          </p>
        )}
      </div>
    </div>
  )
}

/** Joins the two halves the way the server parses them. */
export function joinAddress(local: string, domain: string): string {
  return `${local.trim()}@${domain}`
}

/** Splits a stored address back into the two halves the field edits. */
export function splitAddress(address: string): { local: string; domain: string } {
  const at = address.lastIndexOf('@')
  if (at < 0) return { local: address, domain: '' }
  return { local: address.slice(0, at), domain: address.slice(at + 1) }
}

/**
 * A quota, typed in the unit an operator thinks in.
 *
 * Bytes are what the API stores and what nobody types: `2147483648` is a
 * transcription error waiting to happen. The number and its unit are separate
 * controls, and `0` is spelled out as unlimited rather than left to be guessed.
 */
export function QuotaField({ bytes, onChange, id }: {
  bytes:    number
  onChange: (bytes: number) => void
  id?:      string
}) {
  const { t } = useTranslation('mail')
  const MB = 1024 * 1024
  const GB = 1024 * MB
  const unit = bytes > 0 && bytes % GB === 0 ? 'GB' : 'MB'
  const size = unit === 'GB' ? bytes / GB : Math.round(bytes / MB)

  const set = (value: number, u: string) => {
    const factor = u === 'GB' ? GB : MB
    onChange(Math.max(0, Math.round(value * factor)))
  }

  return (
    <div className="flex flex-wrap items-center gap-2">
      <div className="w-28">
        <NumberInput id={id} value={size} min={0} step={1} onChange={v => set(v, unit)} />
      </div>
      <div className="w-24">
        <Dropdown
          value={unit}
          onChange={u => set(size, u)}
          options={[{ value: 'MB', label: 'Mo' }, { value: 'GB', label: 'Go' }]}
          height={36}
          width="100%"
          focusable
        />
      </div>
      <span className="text-xs text-text-tertiary">
        {bytes > 0
          ? formatSize(bytes)
          : t('addr_quota_unlimited', { defaultValue: '0 = illimité' })}
      </span>
    </div>
  )
}

/**
 * The owner picker, fed by the core directory through the module.
 *
 * A `user_id` typed by hand is a mailbox filed into an account that may not
 * exist — the module cannot read `core.users`, so nothing downstream would
 * catch it; it would surface months later as `owner_known: false`. So the field
 * only ever offers accounts the directory returned.
 */
export function OwnerField({ value, onChange, users, loading, error, id }: {
  value:    string
  onChange: (id: string) => void
  users:    DirectoryUser[]
  loading:  boolean
  error:    string | null
  id?:      string
}) {
  const { t } = useTranslation('mail')

  if (error) {
    return (
      <Callout variant="warning" title={t('addr_dir_failed', { defaultValue: 'Annuaire injoignable' })}>
        {error}
      </Callout>
    )
  }

  const options = users.map(u => ({
    value:       u.id,
    label:       u.display_name || u.username || u.email || u.id,
    description: u.email ?? u.username ?? undefined,
    keywords:    [u.username, u.email, u.id].filter(Boolean).join(' '),
  }))

  return (
    <Combobox
      id={id}
      value={value || null}
      onChange={onChange}
      options={options}
      placeholder={loading
        ? t('addr_dir_loading', { defaultValue: 'Chargement des comptes…' })
        : t('addr_pick_owner', { defaultValue: 'Choisir un compte…' })}
      searchPlaceholder={t('addr_search_owner', { defaultValue: 'Rechercher un compte…' })}
      emptyLabel={t('addr_no_owner_match', { defaultValue: 'Aucun compte ne correspond.' })}
      width="100%"
      aria-label={t('addr_owner', { defaultValue: 'Propriétaire' })}
    />
  )
}

// ── The one-shot password ────────────────────────────────────────────────────

/**
 * The IMAP/SMTP password, shown once and never again.
 *
 * The server does not store it — only its Argon2 hash and SCRAM secret — so
 * this panel is the ONLY moment it exists in readable form. Three consequences
 * are designed in: it is never auto-dismissed (a click beside it must not
 * destroy it), it is selectable and copyable in one gesture, and it says in so
 * many words that closing is final. Losing it is not fatal — the credential can
 * be re-issued — but that changes the password already configured in a mail
 * client, which is a support call.
 */
export function CredentialPanel({ username, password, note, onDone }: {
  username: string
  password: string
  note:     string
  onDone:   () => void
}) {
  const { t } = useTranslation('mail')
  const [copied, setCopied] = useState(false)

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(password)
      setCopied(true)
      window.setTimeout(() => setCopied(false), 2500)
    } catch {
      setCopied(false)
    }
  }

  return (
    <div className="mb-4 rounded-lg border-2 border-warning bg-warning-light p-4">
      <div className="mb-2 flex items-center gap-2">
        <KeyRound size={16} className="text-warning" />
        <h3 className="text-sm font-bold text-text-primary">
          {t('addr_cred_title', { defaultValue: 'Mot de passe IMAP/SMTP — affiché une seule fois' })}
        </h3>
      </div>

      <p className="mb-3 max-w-3xl text-sm leading-relaxed text-text-secondary">
        {note || t('addr_cred_note', {
          defaultValue:
            'Ce mot de passe n’est pas conservé : il ne pourra pas être réaffiché. Notez-le ou transmettez-le maintenant.',
        })}
      </p>

      <div className="mb-2 grid gap-2 sm:grid-cols-[auto_1fr] sm:items-center">
        <span className="text-xs text-text-secondary">
          {t('addr_cred_user', { defaultValue: 'Identifiant' })}
        </span>
        <code className="select-all break-all rounded border border-border bg-surface-0 px-2 py-1 font-mono text-xs text-text-primary">
          {username}
        </code>
        <span className="text-xs text-text-secondary">
          {t('addr_cred_password', { defaultValue: 'Mot de passe' })}
        </span>
        <div className="flex items-center gap-2">
          <code className="min-w-0 flex-1 select-all break-all rounded border border-border bg-surface-0 px-2 py-1 font-mono text-sm text-text-primary">
            {password}
          </code>
          <Button variant="secondary" size="sm" onClick={() => void copy()}
            icon={copied ? <Check size={14} /> : <Copy size={14} />}>
            {copied
              ? t('copied', { defaultValue: 'Copié' })
              : t('copy', { defaultValue: 'Copier' })}
          </Button>
        </div>
      </div>

      <div className="mt-3 flex items-center gap-2">
        <Button variant="secondary" size="sm" onClick={onDone}>
          {t('addr_cred_ack', { defaultValue: 'J’ai noté ce mot de passe' })}
        </Button>
        <span className="inline-flex items-center gap-1 text-xs text-text-tertiary">
          <AlertTriangle size={12} />
          {t('addr_cred_final', { defaultValue: 'Fermer cet encart le fait disparaître définitivement.' })}
        </span>
      </div>
    </div>
  )
}

// ── Occupancy ────────────────────────────────────────────────────────────────

/**
 * Said once, above the table: there is no byte figure, and why.
 *
 * The alternative — a progress bar at 0 % next to every quota — reads as "this
 * mailbox is empty", which is false for a mailbox holding ten years of mail.
 */
export function UsageNote({ note }: { note: string }) {
  const { t } = useTranslation('mail')
  return (
    <p className="mb-3 flex items-start gap-1.5 leading-relaxed text-text-tertiary"
      style={{ fontSize: 'var(--kb-text-micro, 11px)' }}>
      <HelpCircle size={12} className="mt-0.5 shrink-0" />
      <span>
        {note || t('addr_usage_note', {
          defaultValue: 'L’occupation réelle des boîtes n’est pas encore mesurée : seul le quota configuré est affiché.',
        })}
      </span>
    </p>
  )
}
