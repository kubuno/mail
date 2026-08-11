/**
 * The addressing surface of the mail admin panel: mailboxes, aliases, mailing
 * lists and per-domain policy.
 *
 * Kept apart from `../api.ts` (keys and diagnostic) because these routes are a
 * different object: they read and WRITE the addresses this instance owns, they
 * paginate, and every one of them answers 403 to anybody but an administrator.
 *
 * ── The shapes below are the server's, verbatim ──────────────────────────────
 * Three fields are deliberately nullable and must not be flattened into a
 * boolean by the views:
 *  • `used_bytes` is ALWAYS null and `usage_bytes_available` always false —
 *    `mail.messages` stores neither the raw bytes nor a size, so any figure
 *    would be invented. Showing "0 o" would be a lie, not an approximation.
 *  • `domain_served: false` means the domain left `server_domains` and the row
 *    receives nothing; `null` means the core could not be asked. Painting both
 *    the same way sends an operator chasing a fault that is not there.
 *  • `owner_known: false` means the mail schema has never seen that account —
 *    a state to fix, not a detail.
 */
import { api } from '@kubuno/sdk'

// ── Directory ────────────────────────────────────────────────────────────────

export interface DirectoryUser {
  id:           string
  username?:    string | null
  display_name?: string | null
  avatar_url?:  string | null
  email?:       string | null
}

// ── Mailboxes ────────────────────────────────────────────────────────────────

export interface Mailbox {
  id:            string
  address:       string
  domain:        string
  user_id:       string
  display_name:  string | null
  /** 0 = unlimited. */
  quota_bytes:   number
  is_active:     boolean
  comment:       string | null
  created_at:    string
  updated_at:    string
  /** `null` = the core could not be reached, NOT "no". */
  domain_served: boolean | null
  owner_known:   boolean
  has_credential: boolean
  /** Messages of the OWNER account, every address together. */
  owner_message_count: number
  /** Always null — see the file header. */
  used_bytes:    number | null
}

export interface MailboxPage {
  items:  Mailbox[]
  total:  number
  limit:  number
  offset: number
  served_domains: string[] | null
  usage_bytes_available: boolean
  usage_note: string
}

/** The IMAP/SMTP login, returned exactly once — `password` is not stored. */
export interface CreatedCredential {
  id:       string
  username: string
  password: string
  note:     string
}

export interface CreatedMailbox {
  mailbox:    Mailbox
  credential: CreatedCredential | null
}

export interface CreateMailboxDto {
  address:            string
  user_id:            string
  display_name?:      string
  quota_bytes?:       number
  is_active?:         boolean
  comment?:           string
  create_credential?: boolean
  credential_label?:  string
  replace_credential?: boolean
}

export interface UpdateMailboxDto {
  address?:      string
  user_id?:      string
  display_name?: string
  quota_bytes?:  number
  is_active?:    boolean
  comment?:      string
}

/** What the server says it did NOT delete. Shown as-is. */
export interface DeleteMailboxResult {
  deleted:            boolean
  address:            string
  messages_kept:      number
  messages_deleted:   number
  credential_deleted: boolean
  message:            string
}

// ── Aliases ──────────────────────────────────────────────────────────────────

export interface Alias {
  id:           string
  address:      string
  domain:       string
  destinations: string[]
  is_catch_all: boolean
  is_active:    boolean
  comment:      string | null
  created_at:   string
  updated_at:   string
  domain_served: boolean | null
  /** Destinations outside every local domain: only forwarded when outbound
   *  delivery is enabled. Empty when the served domains are unknown. */
  remote_destinations: string[]
}

export interface AliasPage {
  items:  Alias[]
  total:  number
  limit:  number
  offset: number
  served_domains: string[] | null
}

export interface CreateAliasDto {
  address:      string
  destinations: string[]
  is_active?:   boolean
  comment?:     string
}

export interface UpdateAliasDto {
  address?:      string
  destinations?: string[]
  is_active?:    boolean
  comment?:      string
}

// ── Mailing lists ────────────────────────────────────────────────────────────

/** Who may post. `allowed` with an empty allow-list is refused by the server. */
export type PostPolicy = 'anyone' | 'members' | 'internal' | 'allowed'

export interface MailingList {
  id:              string
  address:         string
  domain:          string
  name:            string
  post_policy:     PostPolicy
  allowed_senders: string[]
  is_active:       boolean
  comment:         string | null
  created_at:      string
  updated_at:      string
  members:         string[]
  member_count:    number
  domain_served:   boolean | null
}

export interface MailingListPage {
  items:  MailingList[]
  total:  number
  limit:  number
  offset: number
  served_domains: string[] | null
}

export interface CreateListDto {
  address:          string
  name:             string
  post_policy?:     PostPolicy
  allowed_senders?: string[]
  members?:         string[]
  is_active?:       boolean
  comment?:         string
}

export interface UpdateListDto {
  address?:         string
  name?:            string
  post_policy?:     PostPolicy
  allowed_senders?: string[]
  members?:         string[]
  is_active?:       boolean
  comment?:         string
}

// ── Domains ──────────────────────────────────────────────────────────────────

/**
 * WHY a domain is served — the link between this screen and Instance ▸ Domaines.
 *
 *  • `instance` — the instance declares the domain AND has proved it by DNS.
 *  • `extra`    — served only by this module's stand-in list, for names the
 *                 instance cannot prove (`kubuno.local` on a lab machine).
 *  • `both`     — verified by the instance AND listed in the stand-in list.
 *                 Harmless, and worth saying: the stand-in entry is redundant,
 *                 and it is what would keep the domain served if somebody
 *                 removed it from the instance — silently.
 *  • `null`     — nothing serves it. Mail addressed here is refused.
 */
export type DomainSource = 'instance' | 'extra' | 'both'

/** What Instance ▸ Domaines says about the name, independently of mail. */
export type InstanceDomainStatus = 'verified' | 'pending' | 'absent'

export interface DomainView {
  domain:              string
  /** The only truth about "does mail reach us for this domain". */
  is_served:           boolean
  /**
   * Provenance, added by the server. OPTIONAL on purpose: an instance running
   * an older mail service does not send them, and the views then say nothing
   * about provenance rather than guessing — a wrong origin is worse than none.
   */
  source?:             DomainSource | null
  instance_state?:     InstanceDomainStatus
  has_policy:          boolean
  default_quota_bytes: number
  max_mailboxes:       number
  comment:             string | null
  mailbox_count:       number
  alias_count:         number
  mailing_list_count:  number
  has_catch_all:       boolean
  catch_all_id:        string | null
  created_at:          string | null
  updated_at:          string | null
}

export interface DomainsResponse {
  items:          DomainView[]
  /** `server_domains`, verbatim. */
  served_domains: string[]
}

export interface DomainPolicyDto {
  default_quota_bytes?: number
  max_mailboxes?:       number
  comment?:             string
}

// ── Query shape shared by the three list routes ──────────────────────────────

export interface ListQuery {
  q?:      string
  domain?: string
  active?: boolean
  limit?:  number
  offset?: number
}

/** Drops the keys the server should read as "no filter" — an empty `q=` and a
 *  `domain=` of `''` are not the same request as omitting them. */
function params(q: ListQuery): Record<string, string | number | boolean> {
  const out: Record<string, string | number | boolean> = {}
  if (q.q && q.q.trim())           out.q = q.q.trim()
  if (q.domain)                    out.domain = q.domain
  if (typeof q.active === 'boolean') out.active = q.active
  if (q.limit  !== undefined)      out.limit = q.limit
  if (q.offset !== undefined)      out.offset = q.offset
  return out
}

export const addressesApi = {
  directory: (q: string, limit = 100) =>
    api.get<{ users: DirectoryUser[] }>('/mail/admin/directory/users', {
      params: q.trim() ? { q: q.trim(), limit } : { limit },
    }).then(r => r.data.users ?? []),

  // Mailboxes
  listMailboxes: (q: ListQuery) =>
    api.get<MailboxPage>('/mail/admin/mailboxes', { params: params(q) }).then(r => r.data),
  createMailbox: (dto: CreateMailboxDto) =>
    api.post<CreatedMailbox>('/mail/admin/mailboxes', dto).then(r => r.data),
  updateMailbox: (id: string, dto: UpdateMailboxDto) =>
    api.patch<Mailbox>(`/mail/admin/mailboxes/${id}`, dto).then(r => r.data),
  deleteMailbox: (id: string, deleteCredential: boolean) =>
    api.delete<DeleteMailboxResult>(`/mail/admin/mailboxes/${id}`, {
      params: { delete_credential: deleteCredential },
    }).then(r => r.data),
  issueCredential: (id: string, label?: string) =>
    api.post<CreatedCredential>(`/mail/admin/mailboxes/${id}/credential`, { label })
      .then(r => r.data),

  // Aliases
  listAliases: (q: ListQuery) =>
    api.get<AliasPage>('/mail/admin/aliases', { params: params(q) }).then(r => r.data),
  createAlias: (dto: CreateAliasDto) =>
    api.post<Alias>('/mail/admin/aliases', dto).then(r => r.data),
  updateAlias: (id: string, dto: UpdateAliasDto) =>
    api.patch<Alias>(`/mail/admin/aliases/${id}`, dto).then(r => r.data),
  deleteAlias: (id: string) =>
    api.delete<{ message: string }>(`/mail/admin/aliases/${id}`).then(r => r.data),

  // Mailing lists
  listMailingLists: (q: ListQuery) =>
    api.get<MailingListPage>('/mail/admin/mailing-lists', { params: params(q) }).then(r => r.data),
  createMailingList: (dto: CreateListDto) =>
    api.post<MailingList>('/mail/admin/mailing-lists', dto).then(r => r.data),
  updateMailingList: (id: string, dto: UpdateListDto) =>
    api.patch<MailingList>(`/mail/admin/mailing-lists/${id}`, dto).then(r => r.data),
  deleteMailingList: (id: string) =>
    api.delete<{ message: string }>(`/mail/admin/mailing-lists/${id}`).then(r => r.data),
  setMembers: (id: string, addresses: string[]) =>
    api.put<MailingList>(`/mail/admin/mailing-lists/${id}/members`, { addresses })
      .then(r => r.data),

  // Domains
  listDomains: () =>
    api.get<DomainsResponse>('/mail/admin/domains').then(r => r.data),
  saveDomainPolicy: (domain: string, dto: DomainPolicyDto) =>
    api.put<Record<string, unknown>>(`/mail/admin/domains/${encodeURIComponent(domain)}`, dto)
      .then(r => r.data),
  deleteDomainPolicy: (domain: string) =>
    api.delete<{ message: string }>(`/mail/admin/domains/${encodeURIComponent(domain)}`)
      .then(r => r.data),
}

// ── Query keys ───────────────────────────────────────────────────────────────

export const ADDR_KEYS = {
  mailboxes: 'mail-admin-mailboxes',
  aliases:   'mail-admin-aliases',
  lists:     'mail-admin-lists',
  domains:   'mail-admin-domains',
  directory: 'mail-admin-directory',
} as const

/**
 * The server's own sentence, or the fallback.
 *
 * Every write route answers `{ error, message }` with a message written for an
 * operator — it names the object already holding an address, the domain that is
 * not served, the ceiling that was reached. Replacing it with "Échec" throws
 * away the only part of the response that says what to do.
 *
 * ⚠️ The SDK's axios client does NOT reject with the AxiosError: its response
 * interceptor normalises the failure to a FLAT `{ message, code }`, so the
 * sentence arrives at `error.message` and `error.response` does not exist.
 * Reading only the nested shape is why a caller gets a generic "Création
 * impossible" while the server was naming the exact conflict. Both shapes are
 * accepted here so this keeps working if the client stops normalising.
 */
export function serverMessage(error: unknown, fallback: string): string {
  const nested = (error as { response?: { data?: { message?: string; error?: string } } })
    ?.response?.data
  if (typeof nested?.message === 'string' && nested.message.length > 0) return nested.message

  const flat = error as { message?: unknown }
  if (typeof flat?.message === 'string' && flat.message.length > 0) return flat.message

  if (typeof nested?.error === 'string' && nested.error.length > 0) return nested.error
  return fallback
}
