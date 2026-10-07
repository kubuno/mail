/** Admin-only surface of the mail module: signing keys and the deliverability
 *  report. Kept apart from `src/api.ts` (the user-facing client) because these
 *  routes answer 403 to everyone but an administrator — mixing them would put
 *  calls that always fail in the middle of the mail application. */
import { api } from '@kubuno/sdk'

export interface DkimKey {
  id:         string
  domain:     string
  selector:   string
  /** 'rsa-sha256' | 'ed25519-sha256' */
  algorithm:  string
  /** The key outgoing mail of this domain is signed with. */
  is_active:  boolean
  created_at: string
  /** `<selector>._domainkey.<domain>` */
  dns_name:   string
  /** `v=DKIM1; k=rsa; p=…` */
  dns_value:  string
}

export interface CreateDkimDto {
  domain:     string
  selector?:  string
  algorithm?: string
  activate?:  boolean
}

/** Colour of one diagnostic light. `info` = published, but only the operator
 *  can judge it; `unknown` = the check could not be made (never a failure). */
export type Verdict = 'ok' | 'warn' | 'fail' | 'info' | 'unknown'

export interface DiagnosticCheck {
  id:        string
  /** 'mx' | 'spf' | 'dkim' | 'dmarc' | 'ptr' | 'tls' */
  kind:      string
  scope:     string
  verdict:   Verdict
  summary:   string
  expected:  string | null
  found:     string[]
}

/** One DNS record to publish, prefilled and ready to paste. `value_alt` is set
 *  for DKIM only — the bare public key, for registrars that store it alone. */
export interface DnsRecord {
  domain:    string
  /** 'a' | 'mx' | 'spf' | 'dkim' | 'dmarc' */
  key:       string
  name:      string
  /** 'A' | 'MX' | 'TXT' */
  rtype:     string
  value:     string
  value_alt: string | null
  /** MX only: the preference (10). */
  priority:  number | null
  help:      string
  status:    Verdict
}

export interface DiagnosticReport {
  hostname:   string
  domains:    string[]
  configured: boolean
  checks:     DiagnosticCheck[]
  /** The records to publish, one set per served domain. */
  records:    DnsRecord[]
  /** Generic warnings shown above the records. */
  advisories: string[]
}

/** Outbound relay (smarthost) state, WITHOUT the password. `has_password` is the
 *  only thing said about the secret — it is never returned. */
export interface RelayConfig {
  enabled:      boolean
  host:         string
  port:         number
  /** 'none' | 'starttls' | 'tls' */
  security:     string
  username:     string
  has_password: boolean
}

/** What the PUT accepts. `password` is write-only: omit or leave empty to keep
 *  the stored one; set `clear_password` to wipe it. */
export interface RelayUpdate {
  enabled:         boolean
  host:            string
  port:            number
  security:        string
  username:        string
  password?:       string
  clear_password?: boolean
}

export const mailAdminApi = {
  listDkimKeys: () =>
    api.get<DkimKey[]>('/mail/dkim').then(r => r.data),

  getRelay: () =>
    api.get<RelayConfig>('/mail/admin/relay').then(r => r.data),

  updateRelay: (dto: RelayUpdate) =>
    api.put<RelayConfig>('/mail/admin/relay', dto).then(r => r.data),

  createDkimKey: (dto: CreateDkimDto) =>
    api.post<DkimKey>('/mail/dkim', dto).then(r => r.data),

  activateDkimKey: (id: string) =>
    api.post<DkimKey>(`/mail/dkim/${id}/activate`).then(r => r.data),

  deleteDkimKey: (id: string) =>
    api.delete(`/mail/dkim/${id}`).then(r => r.data),

  diagnostics: () =>
    api.get<DiagnosticReport>('/mail/diagnostics').then(r => r.data),
}
