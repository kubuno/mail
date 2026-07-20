import { api } from '@kubuno/sdk'

export interface EmailAccount {
  id:            string
  user_id:       string
  name:          string
  email_address: string
  incoming_protocol: string
  imap_host:         string
  imap_port:         number
  imap_security:     string
  imap_username:     string
  smtp_host:         string
  smtp_port:     number
  smtp_security: string
  smtp_username: string
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

export interface Label {
  id:          string
  account_id:  string
  name:        string
  color:       string | null
  is_system:   boolean
  position:    number
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

export interface ThreadListParams {
  account_id?: string
  folder?:     string
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

  // Threads
  listThreads: (params: ThreadListParams) =>
    api.get<{ threads: Thread[]; has_more: boolean; cursor: string | null }>('/mail/threads', { params }).then(r => r.data),

  getCounts: () =>
    api.get<MailCounts>('/mail/counts').then(r => r.data),

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

  // Attachment download URL
  attachmentUrl: (messageId: string, index: number) =>
    `/api/v1/mail/messages/${messageId}/attachments/${index}`,

  // Labels
  listLabels: () =>
    api.get<{ labels: Label[] }>('/mail/labels').then(r => r.data),

  createLabel: (dto: { account_id: string; name: string; color?: string }) =>
    api.post<{ id: string }>('/mail/labels', dto).then(r => r.data),

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

  // Anti-spam bayésien
  getSpamStats: () =>
    api.get<SpamStats>('/mail/spam/stats').then(r => r.data),

  updateSpamSettings: (dto: { auto_classify?: boolean; threshold?: number }) =>
    api.patch('/mail/spam/settings', dto).then(r => r.data),

  trainSpam: () =>
    api.post<{ spam_messages: number; ham_messages: number; capped: boolean }>('/mail/spam/train')
      .then(r => r.data),
}
