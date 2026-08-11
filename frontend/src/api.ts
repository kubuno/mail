import { api } from '@kubuno/sdk'

/** Best-effort backend error message from an axios/fetch error, else `fallback`. */
export function apiErrorMessage(e: unknown, fallback: string): string {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const data = (e as any)?.response?.data
  if (data && typeof data === 'object' && typeof data.message === 'string' && data.message.trim()) {
    return data.message
  }
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const m = (e as any)?.message
  return typeof m === 'string' && m.trim() ? m : fallback
}

export interface EmailAccount {
  id:            string
  user_id:       string
  name:          string
  email_address: string
  /**
   * 'external' = user-configured IMAP/SMTP account.
   * 'local'    = mailbox hosted by this instance, assigned by the admin;
   *              no external transport config, sends/receives through the instance.
   */
  kind:          'external' | 'local'
  /** For local accounts, the mail.mailboxes row it is backed by; null otherwise. */
  mailbox_id:    string | null
  incoming_protocol: string
  imap_host:         string
  imap_port:         number
  imap_security:     string
  imap_username:     string
  smtp_host:         string
  smtp_port:     number
  smtp_security: string
  smtp_username: string
  /** 'password' | 'oauth_google' | 'oauth_microsoft' */
  auth_kind:     string
  is_default:    boolean
  is_active:     boolean
  last_sync_at:  string | null
  last_error:    string | null
  created_at:    string
  updated_at:    string
}

export interface CreateAccountDto {
  name:               string
  email_address:      string
  incoming_protocol?: string
  imap_host:          string
  imap_port?:    number
  imap_security?: string
  imap_username: string
  imap_password: string
  smtp_host:     string
  smtp_port?:    number
  smtp_security?: string
  smtp_username: string
  smtp_password: string
  is_default?:   boolean
}

export interface EmailAddress {
  name?:  string
  email: string
}

export interface Thread {
  id:              string
  account_id:      string
  user_id:         string
  subject:         string
  message_count:   number
  unread_count:    number
  has_attachments: boolean
  is_starred:      boolean
  is_important?:   boolean
  snippet:           string | null
  last_sender_name:  string | null
  last_sender_email: string
  last_message_at:   string
  created_at:        string
  /** User labels carried by the thread (chips on the row). */
  labels?:  ThreadLabelRef[]
  /** System folders the thread's messages sit in: inbox, sent, drafts… */
  folders?: string[]
  /** List-Unsubscribe of the newest message, when the sender offers one. */
  list_unsubscribe?: string | null
  /** Category set by dropping the thread on a tab; null = sender heuristic. */
  category?: string | null
  /** Attachments of the newest message, for the chips on the row. */
  attachments?: ThreadAttachment[]
}

export interface ThreadAttachment {
  name:       string
  mime:       string
  size:       number
  message_id: string
  index:      number
}

export interface ThreadLabelRef {
  id:    string
  name:  string
  color: string | null
}

export interface EmailMessage {
  id:            string
  thread_id:     string
  account_id:    string
  user_id:       string
  message_id:    string | null
  from_name:     string | null
  from_email:    string
  to_addresses:  EmailAddress[]
  cc_addresses:  EmailAddress[]
  subject:       string
  body_text:     string | null
  body_html:     string | null
  attachments:   Attachment[]
  is_read:       boolean
  is_starred:    boolean
  folder:        string
  sent_at:       string | null
  received_at:   string
  spam_score?:   number | null
  list_unsubscribe?: string | null
  /** Provenance headers, shown in the details panel. */
  reply_to?:  string | null
  mailed_by?: string | null
  signed_by?: string | null
  security?:  string | null
  /** DMARC verdict: only 'pass' unlocks the sender's brand logo. */
  auth_dmarc?:   string | null
  /** OpenPGP verdict, computed at read time (not stored). */
  pgp_encrypted?:           boolean
  /** Signature verdict: true verified, false present-but-invalid, null unsigned. */
  pgp_signature_valid?:     boolean | null
  pgp_signer_fingerprint?:  string | null
}

export interface SpamStats {
  spam_messages:   number
  ham_messages:    number
  distinct_tokens: number
  auto_classify:   boolean
  threshold:       number
}

export interface Attachment {
  name:         string
  mime:         string
  size:         number
  storage_path: string
}

export interface Draft {
  id:            string
  account_id:    string
  to_addresses:  EmailAddress[]
  cc_addresses:  EmailAddress[]
  bcc_addresses: EmailAddress[]
  subject:       string
  body_html:     string
  reply_to_id:   string | null
  attachments:   Attachment[]
  created_at:    string
  updated_at:    string
}

/** An OpenPGP identity owned by the user (public view — never the secret key). */
export interface PgpKey {
  id:          string
  email:       string | null
  fingerprint: string
  public_key:  string
  is_default:  boolean
  created_at:  string
}

/** A correspondent's stored public key. */
export interface PgpContact {
  id:          string
  email:       string
  fingerprint: string
  public_key:  string
  source:      string
  created_at:  string
}

export interface Label {
  id:          string
  account_id:  string
  name:        string
  color:       string | null
  is_system:   boolean
  position:    number
  /** Sidebar visibility: 'show' | 'unread' | 'hide' */
  list_visibility?:         LabelListVisibility
  /** Chip on message rows: 'show' | 'hide' */
  message_list_visibility?: LabelMsgVisibility
}

export type LabelListVisibility = 'show' | 'unread' | 'hide'
export type LabelMsgVisibility  = 'show' | 'hide'

export interface UpdateLabelDto {
  name?:                    string
  /** `null` clears the color. */
  color?:                   string | null
  list_visibility?:         LabelListVisibility
  message_list_visibility?: LabelMsgVisibility
}

export interface EmailFilter {
  id:               string
  account_id:       string | null
  from_contains:    string | null
  to_contains:      string | null
  subject_contains: string | null
  query_contains:   string | null
  act_archive:      boolean
  act_mark_read:    boolean
  act_star:         boolean
  act_important:    boolean
  act_trash:        boolean
  act_spam:         boolean
  act_label_id:     string | null
  position:         number
  created_at:       string
}

export interface CreateFilterDto {
  account_id?:       string
  from_contains?:    string
  to_contains?:      string
  subject_contains?: string
  query_contains?:   string
  act_archive?:      boolean
  act_mark_read?:    boolean
  act_star?:         boolean
  act_important?:    boolean
  act_trash?:        boolean
  act_spam?:         boolean
  act_label_id?:     string
  apply_existing?:   boolean
}

export interface BlockedSender {
  id:         string
  email:      string
  created_at: string
}

export interface MailCounts {
  unread:    Record<string, number>   // par dossier (inbox, spam, …)
  total:     Record<string, number>   // fils par dossier
  drafts:    number
  starred:   number
  important: number
  snoozed:   number
  scheduled: number
  labels:    Record<string, number>   // par id de libellé
}

/** A provider folder that is not one of the well-known system ones. */
export interface CustomFolder {
  /** Raw provider name — what listings filter on. */
  name:    string
  /** Readable name (modified UTF-7 decoded, "INBOX." prefix dropped). */
  display: string
  total:   number
  unread:  number
}

/** A mailbox credential, as listed — never the secret itself. */
export interface MailboxCredential {
  id:           string
  username:     string
  label:        string | null
  last_used_at: string | null
  created_at:   string
}

export interface ThreadListParams {
  /** Inbox tab: main | social | notifications | promotions. */
  category?: string
  account_id?: string
  folder?:     string
  /** With folder='custom', narrows down to one provider folder. */
  imap_folder?: string
  label_id?:   string
  starred?:    boolean
  important?:  boolean
  snoozed?:    boolean
  unread?:     boolean
  limit?:      number
  before?:     string
  search?:     string
}

export interface Subscription {
  from_email:       string
  from_name:        string | null
  list_unsubscribe: string | null
  count:            number
  last_at:          string
}

export interface ScheduledDraft {
  id:           string
  to_addresses: EmailAddress[]
  subject:      string
  body_html:    string
  scheduled_at: string
}

export interface SendMailDto {
  account_id:    string
  to_addresses:  EmailAddress[]
  cc_addresses?: EmailAddress[]
  bcc_addresses?: EmailAddress[]
  subject:       string
  body_html:     string
  reply_to_id?:  string
  draft_id?:     string
  scheduled_at?: string
  attachments?:  { filename: string; mime: string; content: string }[]
  /** OpenPGP: sign and/or encrypt this message (PGP/MIME). */
  sign?:         boolean
  encrypt?:      boolean
  /** Labels to apply to the Sent copy's thread (only the sender's own ids). */
  label_ids?:    string[]
}

/**
 * Vacation responder (out-of-office auto-reply), as stored on the server.
 * Dates are `YYYY-MM-DD` or `''` (no bound); `messageHtml` is sanitised on save.
 */
export interface VacationConfig {
  enabled:      boolean
  startDate:    string
  endDate:      string
  subject:      string
  messageHtml:  string
  contactsOnly: boolean
}

/**
 * A "send mail as" identity — an address the user has proven they own.
 * The confirmation code never reaches the client; only the verified flag does.
 */
export interface SendAsAddress {
  id:            string
  email:         string
  displayName:   string
  verified:      boolean
  treatAsAlias:  boolean
  createdAt:     string
}

/**
 * An account delegation (Gmail-style "grant access to your account").
 * 'pending'  — invited, not yet accepted (no access).
 * 'accepted' — active (the delegate may read and, if canSend, send).
 * 'revoked'  — ended by either party (no access).
 */
export interface Delegation {
  id:             string
  grantorUserId:  string
  grantorEmail:   string
  delegateUserId: string
  delegateEmail:  string
  status:         'pending' | 'accepted' | 'revoked'
  canSend:        boolean
  createdAt:      string
  acceptedAt:     string | null
}

/** One forwarding destination. */
export interface ForwardAddressConfig {
  email:   string
  enabled: boolean
}

/** Server-side forwarding rules (the POP/IMAP policy is a separate endpoint). */
export interface ForwardingConfig {
  forwardAddresses: ForwardAddressConfig[]
  forwardKeep:      boolean
}

/**
 * POP/IMAP access policy, enforced by the instance's own IMAP/POP3 server.
 * `popState` folds enable + mode into one control ("disabled" / "all" /
 * "from_now"), matching Gmail's radio group.
 */
export interface PopImapSettings {
  imapEnabled:     boolean
  imapExpunge:     'auto' | 'wait'
  imapPurge:       'archive' | 'trash' | 'delete'
  imapFolderLimit: string        // '0' = unlimited, else max messages per folder
  popState:        'disabled' | 'all' | 'from_now'
  popOnFetch:      'keep' | 'mark_read' | 'archive' | 'delete'
}

/** A reusable e-mail template (canned draft), owned by the user. camelCase wire
 *  shape, round-tripped as-is by the compose menu. */
export interface MailTemplate {
  id:        string
  name:      string
  subject:   string
  bodyHtml:  string
  createdAt: string
  updatedAt: string
}

/** One member of a recipient group. Mirrors `EmailAddress` on the wire. */
export interface RecipientGroupMember {
  email: string
  name?: string
}

/** A personal distribution list (« liste de diffusion »). */
export interface RecipientGroup {
  id:        string
  name:      string
  members:   RecipientGroupMember[]
  createdAt: string
  updatedAt: string
}

// ── API client ────────────────────────────────────────────────────────────────

export const mailApi = {
  // Accounts
  listAccounts: () =>
    api.get<{ accounts: EmailAccount[] }>('/mail/accounts').then(r => r.data),

  createAccount: (dto: CreateAccountDto) =>
    api.post<{ id: string }>('/mail/accounts', dto).then(r => r.data),

  getAccount: (id: string) =>
    api.get<EmailAccount>(`/mail/accounts/${id}`).then(r => r.data),

  updateAccount: (id: string, dto: Partial<CreateAccountDto>) => {
    const body = { ...dto }
    if (!body.imap_password) delete body.imap_password
    if (!body.smtp_password) delete body.smtp_password
    return api.patch(`/mail/accounts/${id}`, body).then(r => r.data)
  },

  deleteAccount: (id: string) =>
    api.delete(`/mail/accounts/${id}`).then(r => r.data),

  testConnection: (dto: Omit<CreateAccountDto, 'name' | 'email_address' | 'is_default'>) =>
    api.post<{
      incoming: {
        protocol:   string
        connection: { ok: boolean; error: string | null }
        auth:       { ok: boolean; error: string | null }
      }
      smtp: {
        connection: { ok: boolean; error: string | null }
        auth:       { ok: boolean; error: string | null }
      }
    }>('/mail/accounts/test', dto).then(r => r.data),

  testExistingAccount: (id: string, dto: Omit<CreateAccountDto, 'name' | 'email_address' | 'is_default'>) =>
    api.post<{
      incoming: {
        protocol:   string
        connection: { ok: boolean; error: string | null }
        auth:       { ok: boolean; error: string | null }
      }
      smtp: {
        connection: { ok: boolean; error: string | null }
        auth:       { ok: boolean; error: string | null }
      }
    }>(`/mail/accounts/${id}/test`, dto).then(r => r.data),

  triggerSync: (id: string) =>
    api.post(`/mail/accounts/${id}/sync`).then(r => r.data),

  // OAuth2 (Gmail / Microsoft) account connection
  oauthProviders: () =>
    api.get<{ google: boolean; microsoft: boolean }>('/mail/oauth/providers').then(r => r.data),

  oauthStart: (provider: 'google' | 'microsoft') =>
    api.post<{ auth_url: string }>(`/mail/oauth/${provider}/start`).then(r => r.data),

  // Threads
  listThreads: (params: ThreadListParams) =>
    api.get<{ threads: Thread[]; has_more: boolean; cursor: string | null; total: number | null }>('/mail/threads', { params }).then(r => r.data),

  getCounts: () =>
    api.get<MailCounts>('/mail/counts').then(r => r.data),

  /** The account's own IMAP folders (everything that is not a system folder). */
  listCustomFolders: () =>
    api.get<CustomFolder[]>('/mail/folders').then(r => r.data),

  // Mailbox credentials — what a mail client uses to reach the SMTP/IMAP/POP3
  // services this instance offers. The password only exists in the create call.
  listMailboxCredentials: () =>
    api.get<MailboxCredential[]>('/mail/mailbox-credentials').then(r => r.data),

  createMailboxCredential: (dto: { username: string; label?: string }) =>
    api.post<{ id: string; username: string; password: string }>('/mail/mailbox-credentials', dto).then(r => r.data),

  deleteMailboxCredential: (id: string) =>
    api.delete(`/mail/mailbox-credentials/${id}`).then(r => r.data),

  // Autocomplétion des destinataires (index d'adresses côté mail).
  suggestAddresses: (q: string) =>
    api.get<{ email: string; name: string | null }[]>('/mail/addresses', { params: { q } }).then(r => r.data),

  getThread: (id: string) =>
    api.get<{ thread: Thread; messages: EmailMessage[] }>(`/mail/threads/${id}`).then(r => r.data),

  starThread: (id: string) =>
    api.post<{ is_starred: boolean }>(`/mail/threads/${id}/star`).then(r => r.data),

  importantThread: (id: string) =>
    api.post<{ is_important: boolean }>(`/mail/threads/${id}/important`).then(r => r.data),

  snoozeThread: (id: string, until: string | null) =>
    api.post<{ snoozed_until: string | null }>(`/mail/threads/${id}/snooze`, { until }).then(r => r.data),

  readThread: (id: string, isRead: boolean) =>
    api.post<{ unread_count: number }>(`/mail/threads/${id}/read`, { is_read: isRead }).then(r => r.data),

  muteThread: (id: string) =>
    api.post<{ is_muted: boolean }>(`/mail/threads/${id}/mute`).then(r => r.data),

  getSubscriptions: () =>
    api.get<{ subscriptions: Subscription[] }>('/mail/subscriptions').then(r => r.data.subscriptions),

  getScheduled: () =>
    api.get<{ scheduled: ScheduledDraft[] }>('/mail/scheduled').then(r => r.data.scheduled),

  setThreadCategory: (id: string, category: string | null) =>
    api.post(`/mail/threads/${id}/category`, { category }).then(r => r.data),

  moveThread: (id: string, folder: string) =>
    api.post(`/mail/threads/${id}/move`, { folder }).then(r => r.data),

  deleteThread: (id: string) =>
    api.delete(`/mail/threads/${id}`).then(r => r.data),

  addLabel: (threadId: string, labelId: string) =>
    api.post(`/mail/threads/${threadId}/labels/${labelId}`).then(r => r.data),

  removeLabel: (threadId: string, labelId: string) =>
    api.delete(`/mail/threads/${threadId}/labels/${labelId}`).then(r => r.data),

  // Messages
  getMessage: (id: string) =>
    api.get<EmailMessage>(`/mail/messages/${id}`).then(r => r.data),

  sendMail: (dto: SendMailDto) =>
    api.post('/mail/send', dto).then(r => r.data),

  starMessage: (id: string) =>
    api.post<{ is_starred: boolean }>(`/mail/messages/${id}/star`).then(r => r.data),

  markRead: (id: string, isRead: boolean) =>
    api.patch(`/mail/messages/${id}/read`, { is_read: isRead }).then(r => r.data),

  deleteMessage: (id: string) =>
    api.delete(`/mail/messages/${id}`).then(r => r.data),

  // Drafts
  listDrafts: () =>
    api.get<{ drafts: Draft[] }>('/mail/drafts').then(r => r.data),

  saveDraft: (dto: Partial<SendMailDto> & { account_id: string }) =>
    api.post<{ id: string }>('/mail/drafts', dto).then(r => r.data),

  updateDraft: (id: string, dto: Partial<SendMailDto> & { account_id: string }) =>
    api.patch(`/mail/drafts/${id}`, dto).then(r => r.data),

  deleteDraft: (id: string) =>
    api.delete(`/mail/drafts/${id}`).then(r => r.data),

  // OpenPGP / GPG — key management (Wave 1)
  pgpStatus: () =>
    api.get<{ enabled: boolean }>('/mail/pgp/status').then(r => r.data),
  pgpListKeys: () =>
    api.get<{ keys: PgpKey[] }>('/mail/pgp/keys').then(r => r.data),
  pgpGenerateKey: (dto: { name?: string; email: string }) =>
    api.post<PgpKey>('/mail/pgp/keys/generate', dto).then(r => r.data),
  pgpImportKey: (dto: { secret_armored: string; passphrase?: string }) =>
    api.post<PgpKey>('/mail/pgp/keys/import', dto).then(r => r.data),
  pgpDeleteKey: (id: string) =>
    api.delete(`/mail/pgp/keys/${id}`).then(r => r.data),
  pgpListContacts: () =>
    api.get<{ contacts: PgpContact[] }>('/mail/pgp/contacts').then(r => r.data),
  pgpAddContact: (dto: { email?: string; public_armored: string }) =>
    api.post<PgpContact>('/mail/pgp/contacts', dto).then(r => r.data),
  pgpDeleteContact: (id: string) =>
    api.delete(`/mail/pgp/contacts/${id}`).then(r => r.data),

  // Attachment download URL
  attachmentUrl: (messageId: string, index: number) =>
    `/api/v1/mail/messages/${messageId}/attachments/${index}`,

  // Labels
  listLabels: () =>
    api.get<{ labels: Label[] }>('/mail/labels').then(r => r.data),

  createLabel: (dto: { account_id: string; name: string; color?: string }) =>
    api.post<{ id: string }>('/mail/labels', dto).then(r => r.data),

  updateLabel: (id: string, dto: UpdateLabelDto) =>
    api.patch(`/mail/labels/${id}`, dto).then(r => r.data),

  deleteLabel: (id: string) =>
    api.delete(`/mail/labels/${id}`).then(r => r.data),

  // Filtres / règles automatiques
  listFilters: () =>
    api.get<{ filters: EmailFilter[] }>('/mail/filters').then(r => r.data.filters),

  createFilter: (dto: CreateFilterDto) =>
    api.post<{ id: string }>('/mail/filters', dto).then(r => r.data),

  deleteFilter: (id: string) =>
    api.delete(`/mail/filters/${id}`).then(r => r.data),

  // Adresses bloquées
  listBlocked: () =>
    api.get<{ blocked: BlockedSender[] }>('/mail/blocked').then(r => r.data.blocked),

  blockSender: (email: string) =>
    api.post<{ email: string }>('/mail/blocked', { email }).then(r => r.data),

  unblockSender: (id: string) =>
    api.delete(`/mail/blocked/${id}`).then(r => r.data),

  // Répondeur d'absence (réponse automatique)
  getVacation: () =>
    api.get<VacationConfig>('/mail/vacation').then(r => r.data),

  saveVacation: (dto: VacationConfig) =>
    api.put<{ ok: boolean }>('/mail/vacation', dto).then(r => r.data),

  // « Envoyer en tant que » — identités expéditeur vérifiées par propriété.
  listSendAs: () =>
    api.get<SendAsAddress[]>('/mail/send-as').then(r => r.data),

  addSendAs: (dto: { email: string; displayName?: string }) =>
    api.post<SendAsAddress>('/mail/send-as', dto).then(r => r.data),

  resendSendAs: (id: string) =>
    api.post<{ ok: boolean }>(`/mail/send-as/${id}/resend`).then(r => r.data),

  verifySendAs: (id: string, code: string) =>
    api.post<{ verified: boolean }>(`/mail/send-as/${id}/verify`, { code }).then(r => r.data),

  deleteSendAs: (id: string) =>
    api.delete(`/mail/send-as/${id}`).then(r => r.data),

  // ── Account delegation (Gmail-style "grant access to your account") ────────
  // Delegations I have granted (as grantor), any status.
  listDelegations: () =>
    api.get<Delegation[]>('/mail/delegations').then(r => r.data),

  // Grant a user of this instance access to my mailbox (starts `pending`).
  addDelegation: (email: string) =>
    api.post<Delegation>('/mail/delegations', { email }).then(r => r.data),

  // I (the grantor) revoke a delegation I granted.
  revokeDelegation: (id: string) =>
    api.delete(`/mail/delegations/${id}`).then(r => r.data),

  // Accounts I may currently act on (as an accepted delegate).
  listIncomingDelegations: () =>
    api.get<Delegation[]>('/mail/delegations/incoming').then(r => r.data),

  // I (the delegate) accept / decline an invitation.
  acceptDelegation: (id: string) =>
    api.post<{ ok: boolean }>(`/mail/delegations/${id}/accept`).then(r => r.data),

  declineDelegation: (id: string) =>
    api.post<{ ok: boolean }>(`/mail/delegations/${id}/decline`).then(r => r.data),

  // Transfert automatique
  getForwarding: () =>
    api.get<ForwardingConfig>('/mail/forwarding').then(r => r.data),

  saveForwarding: (dto: ForwardingConfig) =>
    api.put<{ ok: boolean }>('/mail/forwarding', dto).then(r => r.data),

  // Politique POP/IMAP (appliquée par le serveur IMAP/POP3 de l'instance)
  getPopImap: () =>
    api.get<PopImapSettings>('/mail/pop-imap').then(r => r.data),

  savePopImap: (dto: PopImapSettings) =>
    api.put<{ ok: boolean }>('/mail/pop-imap', dto).then(r => r.data),

  // Anti-spam bayésien
  getSpamStats: () =>
    api.get<SpamStats>('/mail/spam/stats').then(r => r.data),

  updateSpamSettings: (dto: { auto_classify?: boolean; threshold?: number }) =>
    api.patch('/mail/spam/settings', dto).then(r => r.data),

  trainSpam: () =>
    api.post<{ spam_messages: number; ham_messages: number; capped: boolean }>('/mail/spam/train')
      .then(r => r.data),

  // ── Modèles (canned drafts) ─────────────────────────────────────────────────
  // Backend returns a bare array (Vec<TemplateDto>); create/update return one.
  listTemplates: () =>
    api.get<MailTemplate[]>('/mail/templates').then(r => r.data),

  createTemplate: (dto: { name: string; subject?: string; bodyHtml?: string }) =>
    api.post<MailTemplate>('/mail/templates', dto).then(r => r.data),

  updateTemplate: (id: string, dto: { name: string; subject?: string; bodyHtml?: string }) =>
    api.put<MailTemplate>(`/mail/templates/${id}`, dto).then(r => r.data),

  deleteTemplate: (id: string) =>
    api.delete(`/mail/templates/${id}`).then(r => r.data),

  // ── Groupes de destinataires (listes de diffusion) ──────────────────────────
  listRecipientGroups: () =>
    api.get<RecipientGroup[]>('/mail/recipient-groups').then(r => r.data),

  createRecipientGroup: (dto: { name: string; members: RecipientGroupMember[] }) =>
    api.post<RecipientGroup>('/mail/recipient-groups', dto).then(r => r.data),

  updateRecipientGroup: (id: string, dto: { name: string; members: RecipientGroupMember[] }) =>
    api.put<RecipientGroup>(`/mail/recipient-groups/${id}`, dto).then(r => r.data),

  deleteRecipientGroup: (id: string) =>
    api.delete(`/mail/recipient-groups/${id}`).then(r => r.data),
}
