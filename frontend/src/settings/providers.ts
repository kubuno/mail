// ── Mail providers ────────────────────────────────────────────────────────────
// Well-known IMAP/POP3/SMTP presets used to pre-fill the account form.

export interface MailProvider {
  id:             string
  label:          string
  imap_host:      string
  imap_port:      number
  imap_security:  string
  pop3_host?:     string
  pop3_port?:     number
  pop3_security?: string
  smtp_host:      string
  smtp_port:      number
  smtp_security:  string
  note?:          string  // i18n key
  no_pop3?:       boolean
  // Provider issues "app passwords" (Gmail, Yahoo, iCloud…) that it DISPLAYS in
  // space-separated groups ("xxxx xxxx xxxx xxxx") but REJECTS on IMAP/SMTP when
  // the spaces are kept. When true, the form strips whitespace from the password.
  app_password?:  boolean
  // Provider supports OAuth2 (XOAUTH2) sign-in when the server admin configured
  // a client id/secret for it (checked via GET /mail/oauth/providers).
  oauth?: 'google' | 'microsoft'
}

export const PROVIDERS: MailProvider[] = [
  {
    id: 'gmail', label: 'Gmail',
    imap_host: 'imap.gmail.com',       imap_port: 993,  imap_security: 'ssl',
    pop3_host: 'pop.gmail.com',        pop3_port: 995,  pop3_security: 'ssl',
    smtp_host: 'smtp.gmail.com',       smtp_port: 587,  smtp_security: 'starttls',
    note: 'mail_settings_note_gmail', app_password: true, oauth: 'google',
  },
  {
    id: 'outlook', label: 'Outlook / Hotmail',
    imap_host: 'outlook.office365.com', imap_port: 993, imap_security: 'ssl',
    pop3_host: 'outlook.office365.com', pop3_port: 995, pop3_security: 'ssl',
    smtp_host: 'smtp.office365.com',    smtp_port: 587, smtp_security: 'starttls',
    oauth: 'microsoft',
  },
  {
    id: 'yahoo', label: 'Yahoo Mail',
    imap_host: 'imap.mail.yahoo.com',  imap_port: 993,  imap_security: 'ssl',
    pop3_host: 'pop.mail.yahoo.com',   pop3_port: 995,  pop3_security: 'ssl',
    smtp_host: 'smtp.mail.yahoo.com',  smtp_port: 465,  smtp_security: 'ssl',
    note: 'mail_settings_note_app_password', app_password: true,
  },
  {
    id: 'icloud', label: 'iCloud',
    imap_host: 'imap.mail.me.com',     imap_port: 993,  imap_security: 'ssl',
    smtp_host: 'smtp.mail.me.com',     smtp_port: 587,  smtp_security: 'starttls',
    note: 'mail_settings_note_icloud', app_password: true,
    no_pop3: true,
  },
  {
    id: 'protonmail', label: 'ProtonMail Bridge',
    imap_host: '127.0.0.1',            imap_port: 1143, imap_security: 'starttls',
    pop3_host: '127.0.0.1',            pop3_port: 1995, pop3_security: 'ssl',
    smtp_host: '127.0.0.1',            smtp_port: 1025, smtp_security: 'starttls',
    note: 'mail_settings_note_protonmail',
  },
  {
    id: 'fastmail', label: 'Fastmail',
    imap_host: 'imap.fastmail.com',    imap_port: 993,  imap_security: 'ssl',
    pop3_host: 'pop.fastmail.com',     pop3_port: 995,  pop3_security: 'ssl',
    smtp_host: 'smtp.fastmail.com',    smtp_port: 465,  smtp_security: 'ssl',
  },
  {
    id: 'ovh', label: 'OVHcloud',
    imap_host: 'imap.mail.ovh.net',    imap_port: 993,  imap_security: 'ssl',
    pop3_host: 'pop.mail.ovh.net',     pop3_port: 995,  pop3_security: 'ssl',
    smtp_host: 'smtp.mail.ovh.net',    smtp_port: 587,  smtp_security: 'starttls',
  },
  {
    id: 'infomaniak', label: 'Infomaniak',
    imap_host: 'mail.infomaniak.com',  imap_port: 993,  imap_security: 'ssl',
    pop3_host: 'mail.infomaniak.com',  pop3_port: 995,  pop3_security: 'ssl',
    smtp_host: 'mail.infomaniak.com',  smtp_port: 587,  smtp_security: 'starttls',
  },
  {
    id: 'zoho', label: 'Zoho Mail',
    imap_host: 'imap.zoho.eu',         imap_port: 993,  imap_security: 'ssl',
    pop3_host: 'pop.zoho.eu',          pop3_port: 995,  pop3_security: 'ssl',
    smtp_host: 'smtp.zoho.eu',         smtp_port: 587,  smtp_security: 'starttls',
  },
]

/** Map an incoming host back to a known provider id ('custom' when unknown). */
export function detectProvider(imapHost: string): string {
  return PROVIDERS.find(p => p.imap_host === imapHost || p.pop3_host === imapHost)?.id ?? 'custom'
}
