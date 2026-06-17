import React, { useState } from 'react'
import { Link } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import {
  ArrowLeft, Mail, Plus, Trash2, RefreshCw, Loader2,
  Eye, EyeOff, AlertCircle, Tag, Check, CheckCircle2, XCircle, Wifi, Ban,
} from 'lucide-react'
import { mailApi, type CreateAccountDto, type EmailAccount } from './api'
import { Button, Dropdown, Checkbox, Radio } from '@ui'

// ── Types ─────────────────────────────────────────────────────────────────────

type TabId = 'general' | 'accounts' | 'labels' | 'filters' | 'spam' | 'forwarding'

const TAB_IDS: TabId[] = ['general', 'accounts', 'labels', 'filters', 'spam', 'forwarding']

interface MailPrefs {
  pageSize:         string
  undoDelay:        string
  defaultReply:     string
  showImages:       string
  conversationView: boolean
}

const DEFAULT_PREFS: MailPrefs = {
  pageSize: '25', undoDelay: '5', defaultReply: 'reply',
  showImages: 'always', conversationView: true,
}

function loadPrefs(): MailPrefs {
  try {
    const s = localStorage.getItem('mail-prefs')
    if (s) return { ...DEFAULT_PREFS, ...JSON.parse(s) }
  } catch { /* ignore */ }
  return DEFAULT_PREFS
}

// ── Layout helpers ────────────────────────────────────────────────────────────

function SettingsRow({ label, description, children }: {
  label: string; description?: string; children: React.ReactNode
}) {
  return (
    <div className="flex items-start gap-8 py-4 border-b border-[#e8eaed] last:border-0">
      <div className="w-60 flex-shrink-0">
        <p className="text-sm text-[#202124] font-normal">{label}</p>
        {description && (
          <p className="text-xs text-text-tertiary mt-0.5 leading-relaxed">{description}</p>
        )}
      </div>
      <div className="flex-1">{children}</div>
    </div>
  )
}

function RadioGroup({ options, value, onChange }: {
  name?: string
  options: { value: string; label: string }[]
  value: string
  onChange: (v: string) => void
}) {
  return (
    <div className="space-y-2">
      {options.map(opt => (
        <Radio
          key={opt.value}
          checked={value === opt.value}
          onChange={() => onChange(opt.value)}
          label={opt.label}
        />
      ))}
    </div>
  )
}

// ── Général tab ───────────────────────────────────────────────────────────────

function GeneralTab() {
  const { t } = useTranslation('mail')
  const [prefs, setPrefs] = useState<MailPrefs>(loadPrefs)
  const [saved, setSaved] = useState(false)

  const set = <K extends keyof MailPrefs>(key: K, value: MailPrefs[K]) =>
    setPrefs(p => ({ ...p, [key]: value }))

  const save = () => {
    localStorage.setItem('mail-prefs', JSON.stringify(prefs))
    setSaved(true)
    setTimeout(() => setSaved(false), 2500)
  }

  return (
    <div>
      <SettingsRow label={t('mail_settings_page_size')} description={t('mail_settings_page_size_desc')}>
        <RadioGroup
          name="pageSize"
          value={prefs.pageSize}
          onChange={v => set('pageSize', v)}
          options={[
            { value: '10',  label: t('mail_settings_convs_per_page', { count: 10 }) },
            { value: '25',  label: t('mail_settings_convs_per_page', { count: 25 }) },
            { value: '50',  label: t('mail_settings_convs_per_page', { count: 50 }) },
            { value: '100', label: t('mail_settings_convs_per_page', { count: 100 }) },
          ]}
        />
      </SettingsRow>

      <SettingsRow
        label={t('mail_settings_undo_send')}
        description={t('mail_settings_undo_send_desc')}
      >
        <RadioGroup
          name="undoDelay"
          value={prefs.undoDelay}
          onChange={v => set('undoDelay', v)}
          options={[
            { value: '0',  label: t('mail_settings_disable') },
            { value: '5',  label: t('mail_settings_seconds', { count: 5 }) },
            { value: '10', label: t('mail_settings_seconds', { count: 10 }) },
            { value: '20', label: t('mail_settings_seconds', { count: 20 }) },
            { value: '30', label: t('mail_settings_seconds', { count: 30 }) },
          ]}
        />
      </SettingsRow>

      <SettingsRow label={t('mail_settings_default_reply')}>
        <RadioGroup
          name="defaultReply"
          value={prefs.defaultReply}
          onChange={v => set('defaultReply', v)}
          options={[
            { value: 'reply',     label: t('mail_settings_reply') },
            { value: 'reply_all', label: t('mail_settings_reply_all') },
          ]}
        />
      </SettingsRow>

      <SettingsRow
        label={t('mail_settings_images')}
        description={t('mail_settings_images_desc')}
      >
        <RadioGroup
          name="showImages"
          value={prefs.showImages}
          onChange={v => set('showImages', v)}
          options={[
            { value: 'always', label: t('mail_settings_images_always') },
            { value: 'ask',    label: t('mail_settings_images_ask') },
          ]}
        />
      </SettingsRow>

      <SettingsRow
        label={t('mail_settings_conversation_mode')}
        description={t('mail_settings_conversation_mode_desc')}
      >
        <label className="flex items-center gap-2 cursor-pointer select-none">
          <input
            type="checkbox"
            checked={prefs.conversationView}
            onChange={e => set('conversationView', e.target.checked)}
            className="w-4 h-4 accent-primary"
          />
          <span className="text-sm text-text-primary">{t('mail_settings_conversation_mode_on')}</span>
        </label>
      </SettingsRow>

      <div className="pt-5 flex items-center gap-3">
        <Button onClick={save}>
          {saved
            ? <><Check size={14} className="mr-1.5 inline" />{t('mail_settings_saved')}</>
            : t('mail_settings_save_changes')
          }
        </Button>
        <Button variant="ghost" onClick={() => setPrefs(DEFAULT_PREFS)}>
          {t('common_cancel')}
        </Button>
      </div>
    </div>
  )
}

// ── Mail providers ────────────────────────────────────────────────────────────

interface MailProvider {
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
  note?:          string  // clé i18n
  no_pop3?:       boolean
}

const PROVIDERS: MailProvider[] = [
  {
    id: 'gmail', label: 'Gmail',
    imap_host: 'imap.gmail.com',       imap_port: 993,  imap_security: 'ssl',
    pop3_host: 'pop.gmail.com',        pop3_port: 995,  pop3_security: 'ssl',
    smtp_host: 'smtp.gmail.com',       smtp_port: 587,  smtp_security: 'starttls',
    note: 'mail_settings_note_gmail',
  },
  {
    id: 'outlook', label: 'Outlook / Hotmail',
    imap_host: 'outlook.office365.com', imap_port: 993, imap_security: 'ssl',
    pop3_host: 'outlook.office365.com', pop3_port: 995, pop3_security: 'ssl',
    smtp_host: 'smtp.office365.com',    smtp_port: 587, smtp_security: 'starttls',
  },
  {
    id: 'yahoo', label: 'Yahoo Mail',
    imap_host: 'imap.mail.yahoo.com',  imap_port: 993,  imap_security: 'ssl',
    pop3_host: 'pop.mail.yahoo.com',   pop3_port: 995,  pop3_security: 'ssl',
    smtp_host: 'smtp.mail.yahoo.com',  smtp_port: 465,  smtp_security: 'ssl',
    note: 'mail_settings_note_app_password',
  },
  {
    id: 'icloud', label: 'iCloud',
    imap_host: 'imap.mail.me.com',     imap_port: 993,  imap_security: 'ssl',
    smtp_host: 'smtp.mail.me.com',     smtp_port: 587,  smtp_security: 'starttls',
    note: 'mail_settings_note_icloud',
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

function detectProvider(imapHost: string): string {
  return PROVIDERS.find(p => p.imap_host === imapHost || p.pop3_host === imapHost)?.id ?? 'custom'
}

// ── Account form ──────────────────────────────────────────────────────────────

interface StepResult { ok: boolean; error: string | null }

interface TestResult {
  incoming: {
    protocol:   string
    connection: StepResult
    auth:       StepResult
  }
  smtp: {
    connection: StepResult
    auth:       StepResult
  }
}

function AccountForm({ onClose, existing }: {
  onClose: () => void
  existing?: EmailAccount
}) {
  const { t } = useTranslation('mail')
  const qc = useQueryClient()
  const [showPassImap, setShowPassImap] = useState(false)
  const [showPassSmtp, setShowPassSmtp] = useState(false)
  const [testing,    setTesting]    = useState(false)
  const [testResult, setTestResult] = useState<TestResult | null>(null)
  const [providerId,  setProviderId]  = useState<string>(
    existing ? detectProvider(existing.imap_host) : 'custom'
  )
  const [protocol, setProtocol] = useState<'imap' | 'pop3'>(
    (existing?.incoming_protocol as 'imap' | 'pop3') ?? 'imap'
  )

  function applyProviderAndProtocol(id: string, proto: 'imap' | 'pop3') {
    setProviderId(id)
    setProtocol(proto)
    setTestResult(null)
    const p = PROVIDERS.find(p => p.id === id)
    if (!p) return
    const incoming_host = proto === 'pop3' && p.pop3_host ? p.pop3_host : p.imap_host
    const incoming_port = proto === 'pop3' && p.pop3_port ? p.pop3_port : p.imap_port
    const incoming_sec  = proto === 'pop3' && p.pop3_security ? p.pop3_security : p.imap_security
    setForm(f => ({
      ...f,
      incoming_protocol: proto,
      imap_host:         incoming_host,
      imap_port:         incoming_port,
      imap_security:     incoming_sec,
      // Pré-remplir les usernames avec l'email si encore vides
      imap_username:     f.imap_username || f.email_address,
      smtp_username:     f.smtp_username || f.email_address,
      smtp_host:         p.smtp_host,
      smtp_port:         p.smtp_port,
      smtp_security:     p.smtp_security,
    }))
  }

  // Quand l'email change, mettre à jour les usernames si un provider est sélectionné
  // et que les usernames correspondent encore à l'ancienne valeur de l'email
  function handleEmailChange(email: string) {
    setForm(f => ({
      ...f,
      email_address: email,
      imap_username: (providerId !== 'custom' && (f.imap_username === f.email_address || !f.imap_username)) ? email : f.imap_username,
      smtp_username: (providerId !== 'custom' && (f.smtp_username === f.email_address || !f.smtp_username)) ? email : f.smtp_username,
    }))
  }

  const activeProvider = PROVIDERS.find(p => p.id === providerId)

  const [form, setForm] = useState<CreateAccountDto>({
    name:          existing?.name          ?? '',
    email_address: existing?.email_address ?? '',
    imap_host:     existing?.imap_host     ?? '',
    imap_port:     existing?.imap_port     ?? 993,
    imap_security: existing?.imap_security ?? 'ssl',
    imap_username: existing?.imap_username ?? '',
    imap_password: '',
    smtp_host:     existing?.smtp_host     ?? '',
    smtp_port:     existing?.smtp_port     ?? 587,
    smtp_security: existing?.smtp_security ?? 'starttls',
    smtp_username: existing?.smtp_username ?? '',
    smtp_password: '',
    is_default:    existing?.is_default    ?? false,
  })

  const mut = useMutation({
    mutationFn: () => existing
      ? mailApi.updateAccount(existing.id, form)
      : mailApi.createAccount(form),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['mail-accounts'] })
      onClose()
    },
  })

  async function handleTest() {
    setTesting(true)
    setTestResult(null)
    const params = {
      incoming_protocol: protocol,        // 'imap' | 'pop3' — utilisé par le backend pour choisir le test
      imap_host:         form.imap_host,
      imap_port:         form.imap_port,
      imap_security:     form.imap_security,
      imap_username:     form.imap_username,
      imap_password:     form.imap_password,
      smtp_host:         form.smtp_host,
      smtp_port:         form.smtp_port,
      smtp_security:     form.smtp_security,
      smtp_username:     form.smtp_username,
      smtp_password:     form.smtp_password,
    }
    try {
      const result = existing
        ? await mailApi.testExistingAccount(existing.id, params)
        : await mailApi.testConnection(params)
      setTestResult(result)
    } catch {
      const failed: StepResult = { ok: false, error: t('mail_settings_test_server_error') }
      setTestResult({
        incoming: { protocol, connection: failed, auth: failed },
        smtp:     { connection: failed, auth: failed },
      })
    } finally {
      setTesting(false)
    }
  }

  const textField = (label: string, key: keyof CreateAccountDto, type = 'text') => (
    <div>
      <label className="block text-xs font-medium text-text-secondary mb-1">{label}</label>
      <input
        type={type}
        value={String(form[key] ?? '')}
        onChange={e => setForm(f => ({ ...f, [key]: e.target.value }))}
        className="w-full border border-border rounded-lg px-3 py-2 text-sm text-text-primary
                   focus:outline-none focus:ring-2 focus:ring-primary/30"
      />
    </div>
  )

  return (
    <div className="fixed inset-0 bg-black/30 z-50 flex items-center justify-center p-4">
      <div className="bg-white rounded-2xl shadow-xl w-full max-w-lg max-h-[90vh] overflow-y-auto">
        <div className="px-6 py-4 border-b border-border">
          <h2 className="text-base font-semibold text-text-primary">
            {existing ? t('mail_settings_edit_account') : t('mail_settings_add_email_account')}
          </h2>
        </div>

        <div className="p-6 space-y-5">
          <div className="grid grid-cols-2 gap-4">
            {textField(t('mail_settings_display_name'), 'name')}
            <div>
              <label className="block text-xs font-medium text-text-secondary mb-1">{t('mail_settings_email_address')}</label>
              <input
                type="email"
                value={form.email_address}
                onChange={e => handleEmailChange(e.target.value)}
                className="w-full border border-border rounded-lg px-3 py-2 text-sm text-text-primary
                           focus:outline-none focus:ring-2 focus:ring-primary/30"
              />
            </div>
          </div>

          {/* Provider selector */}
          <div>
            <label className="block text-xs font-medium text-text-secondary mb-1.5">
              {t('mail_settings_provider')}
            </label>
            <Dropdown
              value={providerId}
              onChange={id => applyProviderAndProtocol(id, protocol === 'pop3' && PROVIDERS.find(p => p.id === id)?.no_pop3 ? 'imap' : protocol)}
              options={[
                ...PROVIDERS.map(p => ({ value: p.id, label: p.label })),
                { value: 'custom', label: t('mail_settings_provider_custom') },
              ]}
              width="100%"
              height={34}
              fontSize={14}
            />
            {activeProvider?.note && (
              <p className="mt-1.5 text-xs text-warning flex items-center gap-1">
                <AlertCircle size={11} className="flex-shrink-0" />
                {t(activeProvider.note)}
              </p>
            )}
          </div>

          {/* Incoming mail section (IMAP or POP3) */}
          <div>
            <div className="flex items-center justify-between mb-3">
              <h3 className="text-xs font-semibold text-text-tertiary uppercase tracking-wider">
                {t('mail_settings_incoming_mail')}
              </h3>
              {/* Protocol toggle */}
              <div className="flex items-center bg-surface-2 rounded-lg p-0.5 gap-0.5">
                {(['imap', 'pop3'] as const).map(proto => {
                  const disabled = proto === 'pop3' && activeProvider?.no_pop3
                  return (
                    <button
                      key={proto}
                      type="button"
                      disabled={!!disabled}
                      onClick={() => applyProviderAndProtocol(providerId, proto)}
                      title={disabled ? t('mail_settings_pop3_unavailable') : undefined}
                      className={`px-3 py-1 text-xs font-medium rounded-md transition-colors disabled:opacity-40 disabled:cursor-not-allowed ${
                        protocol === proto
                          ? 'bg-white text-text-primary shadow-sm'
                          : 'text-text-tertiary hover:text-text-secondary'
                      }`}
                    >
                      {proto.toUpperCase()}
                    </button>
                  )
                })}
              </div>
            </div>
            <div className="grid grid-cols-3 gap-3">
              <div className="col-span-2">
                <label className="block text-xs font-medium text-text-secondary mb-1">
                  {t('mail_settings_server_field', { protocol: protocol.toUpperCase() })}
                </label>
                <input
                  type="text"
                  value={form.imap_host}
                  onChange={e => setForm(f => ({ ...f, imap_host: e.target.value }))}
                  className="w-full border border-border rounded-lg px-3 py-2 text-sm text-text-primary
                             focus:outline-none focus:ring-2 focus:ring-primary/30"
                />
              </div>
              <div>
                <label className="block text-xs font-medium text-text-secondary mb-1">{t('mail_settings_port')}</label>
                <input
                  type="number"
                  value={form.imap_port}
                  onChange={e => setForm(f => ({ ...f, imap_port: Number(e.target.value) }))}
                  className="w-full border border-border rounded-lg px-3 py-2 text-sm"
                />
              </div>
            </div>
            <div className="mt-3 grid grid-cols-2 gap-3">
              <div className="flex flex-col gap-1">
                <label className="text-xs font-medium text-text-secondary">{t('mail_settings_security')}</label>
                <Dropdown
                  className="w-full"
                  value={form.imap_security ?? ''}
                  onChange={v => setForm(f => ({ ...f, imap_security: v }))}
                  options={[
                    { value: 'ssl',      label: 'SSL/TLS' },
                    { value: 'starttls', label: 'STARTTLS' },
                    { value: 'none',     label: t('mail_settings_security_none') },
                  ]}
                />
              </div>
              <div>
                <label className="block text-xs font-medium text-text-secondary mb-1">
                  {t('mail_settings_user_field', { protocol: protocol.toUpperCase() })}
                </label>
                <input
                  type="text"
                  value={form.imap_username}
                  onChange={e => setForm(f => ({ ...f, imap_username: e.target.value }))}
                  className="w-full border border-border rounded-lg px-3 py-2 text-sm text-text-primary
                             focus:outline-none focus:ring-2 focus:ring-primary/30"
                />
              </div>
            </div>
            <div className="mt-3">
              <label className="block text-xs font-medium text-text-secondary mb-1">
                {t('mail_settings_password_field', { protocol: protocol.toUpperCase() })}
              </label>
              <div className="relative">
                <input
                  type={showPassImap ? 'text' : 'password'}
                  value={form.imap_password}
                  onChange={e => setForm(f => ({ ...f, imap_password: e.target.value }))}
                  placeholder={existing ? t('mail_settings_unchanged') : ''}
                  className="w-full border border-border rounded-lg px-3 py-2 pr-10 text-sm"
                />
                <button
                  type="button"
                  onClick={() => setShowPassImap(p => !p)}
                  className="absolute right-3 top-1/2 -translate-y-1/2 text-text-tertiary"
                >
                  {showPassImap ? <EyeOff size={14} /> : <Eye size={14} />}
                </button>
              </div>
            </div>
          </div>

          {/* SMTP */}
          <div>
            <h3 className="text-xs font-semibold text-text-tertiary uppercase tracking-wider mb-3">
              {t('mail_settings_outgoing_mail')}
            </h3>
            <div className="grid grid-cols-3 gap-3">
              <div className="col-span-2">{textField(t('mail_settings_smtp_server'), 'smtp_host')}</div>
              <div>
                <label className="block text-xs font-medium text-text-secondary mb-1">{t('mail_settings_port')}</label>
                <input
                  type="number"
                  value={form.smtp_port}
                  onChange={e => setForm(f => ({ ...f, smtp_port: Number(e.target.value) }))}
                  className="w-full border border-border rounded-lg px-3 py-2 text-sm"
                />
              </div>
            </div>
            <div className="mt-3 grid grid-cols-2 gap-3">
              <div className="flex flex-col gap-1">
                <label className="text-xs font-medium text-text-secondary">{t('mail_settings_security')}</label>
                <Dropdown
                  className="w-full"
                  value={form.smtp_security ?? ''}
                  onChange={v => setForm(f => ({ ...f, smtp_security: v }))}
                  options={[
                    { value: 'starttls', label: 'STARTTLS' },
                    { value: 'ssl',      label: 'SSL/TLS' },
                    { value: 'none',     label: t('mail_settings_security_none') },
                  ]}
                />
              </div>
              {textField(t('mail_settings_smtp_user'), 'smtp_username')}
            </div>
            <div className="mt-3">
              <label className="block text-xs font-medium text-text-secondary mb-1">
                {t('mail_settings_smtp_password')}
              </label>
              <div className="relative">
                <input
                  type={showPassSmtp ? 'text' : 'password'}
                  value={form.smtp_password}
                  onChange={e => setForm(f => ({ ...f, smtp_password: e.target.value }))}
                  placeholder={existing ? t('mail_settings_unchanged') : ''}
                  className="w-full border border-border rounded-lg px-3 py-2 pr-10 text-sm"
                />
                <button
                  type="button"
                  onClick={() => setShowPassSmtp(p => !p)}
                  className="absolute right-3 top-1/2 -translate-y-1/2 text-text-tertiary"
                >
                  {showPassSmtp ? <EyeOff size={14} /> : <Eye size={14} />}
                </button>
              </div>
            </div>
          </div>

          <Checkbox
            label={t('mail_settings_default_account')}
            checked={!!form.is_default}
            onChange={v => setForm(f => ({ ...f, is_default: v }))}
          />

          {testResult && (() => {
            const proto = testResult.incoming.protocol.toUpperCase()
            const rows: { label: string; step: StepResult; successText: string }[] = [
              { label: `${proto} — ${t('mail_settings_test_connection')}`,     step: testResult.incoming.connection, successText: t('mail_settings_test_server_reachable') },
              { label: `${proto} — ${t('mail_settings_test_auth')}`,           step: testResult.incoming.auth,       successText: t('mail_settings_test_creds_valid') },
              { label: `SMTP — ${t('mail_settings_test_connection')}`,         step: testResult.smtp.connection,     successText: t('mail_settings_test_server_reachable') },
              { label: `SMTP — ${t('mail_settings_test_auth')}`,               step: testResult.smtp.auth,           successText: t('mail_settings_test_creds_valid') },
            ]
            return (
              <div className="rounded-lg border border-border overflow-hidden text-sm">
                {rows.map(({ label, step, successText }) => (
                  <div
                    key={label}
                    className={`flex items-start gap-2.5 px-3 py-2.5 border-b border-border last:border-0 ${
                      step.ok ? 'bg-success/5' : 'bg-danger/5'
                    }`}
                  >
                    {step.ok
                      ? <CheckCircle2 size={15} className="text-success flex-shrink-0 mt-px" />
                      : <XCircle      size={15} className="text-danger   flex-shrink-0 mt-px" />
                    }
                    <div>
                      <span className="font-medium text-xs">{label}</span>
                      {' · '}
                      <span className={step.ok ? 'text-success' : 'text-danger'}>
                        {step.ok ? successText : (step.error ?? t('mail_settings_test_failed'))}
                      </span>
                    </div>
                  </div>
                ))}
              </div>
            )
          })()}

          {mut.isError && (
            <div className="flex items-center gap-2 text-danger text-sm bg-danger/10 px-3 py-2 rounded-lg">
              <AlertCircle size={14} />
              {t('mail_settings_save_error')}
            </div>
          )}
        </div>

        <div className="px-6 py-4 border-t border-border flex items-center justify-between gap-3">
          <Button
            variant="secondary"
            onClick={handleTest}
            disabled={testing || !form.imap_host || !form.smtp_host || (!existing && (!form.imap_password || !form.smtp_password))}
            icon={testing
              ? <Loader2 size={13} className="animate-spin" />
              : <Wifi size={13} />
            }
          >
            {testing ? t('mail_settings_testing') : t('mail_settings_test_servers')}
          </Button>
          <div className="flex items-center gap-3">
            <Button variant="ghost" onClick={onClose}>
              {t('common_cancel')}
            </Button>
            <Button onClick={() => mut.mutate()} loading={mut.isPending}>
              {existing ? t('common_save') : t('mail_settings_add')}
            </Button>
          </div>
        </div>
      </div>
    </div>
  )
}

// ── Comptes et importation tab ────────────────────────────────────────────────

function AccountsTab() {
  const { t, i18n } = useTranslation('mail')
  const qc = useQueryClient()
  const [showForm,    setShowForm]    = useState(false)
  const [editAccount, setEditAccount] = useState<EmailAccount | null>(null)

  const { data, isLoading } = useQuery({
    queryKey: ['mail-accounts'],
    queryFn:  mailApi.listAccounts,
  })

  const deleteMut = useMutation({
    mutationFn: (id: string) => mailApi.deleteAccount(id),
    onSuccess:  () => qc.invalidateQueries({ queryKey: ['mail-accounts'] }),
  })

  const syncMut = useMutation({
    mutationFn: (id: string) => mailApi.triggerSync(id),
  })

  const accounts = data?.accounts ?? []

  return (
    <div>
      <div className="flex items-center justify-between mb-5">
        <p className="text-sm text-text-tertiary">
          {t('mail_settings_accounts_configured', { count: accounts.length })}
        </p>
        <Button
          size="sm"
          icon={<Plus size={14} />}
          onClick={() => { setEditAccount(null); setShowForm(true) }}
        >
          {t('mail_settings_add_account')}
        </Button>
      </div>

      {isLoading ? (
        <div className="flex justify-center py-8">
          <Loader2 size={20} className="animate-spin text-text-tertiary" />
        </div>
      ) : accounts.length === 0 ? (
        <div className="text-center py-12">
          <Mail size={32} className="opacity-30 mx-auto mb-3 text-text-tertiary" />
          <p className="text-sm text-text-tertiary font-medium">{t('mail_settings_no_accounts')}</p>
          <p className="text-xs text-text-tertiary mt-1">
            {t('mail_settings_no_accounts_desc')}
          </p>
        </div>
      ) : (
        <div className="space-y-3">
          {accounts.map(account => (
            <div
              key={account.id}
              className="border border-border rounded-xl p-4 hover:shadow-sm transition-shadow bg-white"
            >
              <div className="flex items-start justify-between gap-3">
                <div className="flex-1 min-w-0">
                  <div className="flex items-center gap-2 mb-1">
                    <p className="font-medium text-text-primary text-sm">{account.name}</p>
                    {account.is_default && (
                      <span className="text-xs bg-primary/10 text-primary px-1.5 py-0.5 rounded-full">
                        {t('mail_settings_badge_default')}
                      </span>
                    )}
                    {!account.is_active && (
                      <span className="text-xs bg-warning/10 text-warning px-1.5 py-0.5 rounded-full">
                        {t('mail_settings_badge_inactive')}
                      </span>
                    )}
                  </div>
                  <p className="text-sm text-text-secondary">{account.email_address}</p>
                  <p className="text-xs text-text-tertiary mt-1">
                    {account.incoming_protocol?.toUpperCase() ?? 'IMAP'}: {account.imap_host}:{account.imap_port} ·{' '}
                    SMTP: {account.smtp_host}:{account.smtp_port}
                  </p>
                  {account.last_sync_at && (
                    <p className="text-xs text-text-tertiary mt-0.5">
                      {t('mail_settings_last_sync')}{' '}
                      {new Date(account.last_sync_at).toLocaleString(i18n.language)}
                    </p>
                  )}
                  {account.last_error && (
                    <div className="flex items-center gap-1 mt-1 text-xs text-danger">
                      <AlertCircle size={11} />
                      {account.last_error}
                    </div>
                  )}
                </div>
                <div className="flex items-center gap-1">
                  <button
                    onClick={() => syncMut.mutate(account.id)}
                    disabled={syncMut.isPending}
                    title={t('mail_settings_sync_now')}
                    className="p-1.5 rounded-lg text-text-tertiary hover:bg-surface-2"
                  >
                    <RefreshCw
                      size={14}
                      className={syncMut.isPending ? 'animate-spin' : ''}
                    />
                  </button>
                  <button
                    onClick={() => { setEditAccount(account); setShowForm(true) }}
                    className="px-2 py-1.5 rounded-lg text-text-secondary hover:bg-surface-2 text-xs font-medium"
                  >
                    {t('common_edit')}
                  </button>
                  <button
                    onClick={() => deleteMut.mutate(account.id)}
                    disabled={deleteMut.isPending}
                    className="p-1.5 rounded-lg text-text-tertiary hover:text-danger hover:bg-danger/10"
                  >
                    <Trash2 size={14} />
                  </button>
                </div>
              </div>
            </div>
          ))}
        </div>
      )}

      {showForm && (
        <AccountForm
          existing={editAccount ?? undefined}
          onClose={() => { setShowForm(false); setEditAccount(null) }}
        />
      )}
    </div>
  )
}

// ── Libellés tab ──────────────────────────────────────────────────────────────

const LABEL_COLORS = [
  '#1a73e8', '#e8711a', '#0f9d58', '#d93025',
  '#9c27b0', '#f9ab00', '#00838f', '#6d4c41',
]

function LabelsTab() {
  const { t } = useTranslation('mail')
  const qc = useQueryClient()
  const [newName,    setNewName]    = useState('')
  const [newColor,   setNewColor]   = useState(LABEL_COLORS[0])
  const [showCreate, setShowCreate] = useState(false)
  const [accountId,  setAccountId]  = useState<string | null>(null)

  const accountsQ = useQuery({ queryKey: ['mail-accounts'], queryFn: mailApi.listAccounts })
  const labelsQ   = useQuery({ queryKey: ['mail-labels'],   queryFn: mailApi.listLabels   })

  const accounts = accountsQ.data?.accounts ?? []
  const labels   = labelsQ.data?.labels     ?? []

  const selectedAccountId = accountId ?? accounts[0]?.id ?? null

  const createMut = useMutation({
    mutationFn: () => mailApi.createLabel({
      account_id: selectedAccountId!,
      name:       newName.trim(),
      color:      newColor,
    }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['mail-labels'] })
      setNewName('')
      setShowCreate(false)
    },
  })

  const deleteMut = useMutation({
    mutationFn: (id: string) => mailApi.deleteLabel(id),
    onSuccess:  () => qc.invalidateQueries({ queryKey: ['mail-labels'] }),
  })

  if (labelsQ.isLoading) {
    return (
      <div className="flex justify-center py-8">
        <Loader2 size={20} className="animate-spin text-text-tertiary" />
      </div>
    )
  }

  return (
    <div>
      <div className="flex items-center justify-between mb-5">
        <p className="text-sm text-text-tertiary">
          {t('mail_settings_labels_count', { count: labels.length })}
        </p>
        {accounts.length > 0 && (
          <button
            onClick={() => setShowCreate(true)}
            className="flex items-center gap-1.5 text-sm text-primary hover:underline"
          >
            <Plus size={14} />
            {t('mail_settings_new_label')}
          </button>
        )}
      </div>

      {labels.length === 0 && !showCreate ? (
        <div className="text-center py-12">
          <Tag size={32} className="opacity-30 mx-auto mb-3 text-text-tertiary" />
          <p className="text-sm text-text-tertiary">{t('mail_settings_no_labels')}</p>
        </div>
      ) : (
        <div>
          {labels.map(label => (
            <div
              key={label.id}
              className="flex items-center justify-between py-2.5 border-b border-[#e8eaed] last:border-0"
            >
              <div className="flex items-center gap-3">
                <span
                  className="w-3 h-3 rounded-full flex-shrink-0"
                  style={{ background: label.color ?? '#5f6368' }}
                />
                <span className="text-sm text-text-primary">{label.name}</span>
                {label.is_system && (
                  <span className="text-xs text-text-tertiary bg-surface-2 px-1.5 py-0.5 rounded">
                    {t('mail_settings_label_system')}
                  </span>
                )}
              </div>
              {!label.is_system && (
                <button
                  onClick={() => deleteMut.mutate(label.id)}
                  className="p-1 rounded text-text-tertiary hover:text-danger hover:bg-danger/10"
                >
                  <Trash2 size={13} />
                </button>
              )}
            </div>
          ))}
        </div>
      )}

      {showCreate && (
        <div className="mt-4 p-4 border border-border rounded-xl bg-surface-1">
          <h3 className="text-sm font-medium text-text-primary mb-3">{t('mail_settings_new_label')}</h3>
          <div className="space-y-3">
            {accounts.length > 1 && (
              <div>
                <label className="block text-xs font-medium text-text-secondary mb-1">{t('mail_settings_account')}</label>
                <Dropdown
                  className="w-full"
                  value={selectedAccountId ?? ''}
                  onChange={v => setAccountId(v)}
                  options={accounts.map(a => ({ value: a.id, label: a.name }))}
                />
              </div>
            )}
            <div>
              <label className="block text-xs text-text-secondary mb-1">{t('mail_settings_name')}</label>
              <input
                type="text"
                value={newName}
                onChange={e => setNewName(e.target.value)}
                placeholder={t('mail_settings_label_name_placeholder')}
                autoFocus
                className="w-full border border-border rounded-lg px-3 py-2 text-sm
                           focus:outline-none focus:ring-2 focus:ring-primary/30"
              />
            </div>
            <div>
              <label className="block text-xs text-text-secondary mb-1">{t('mail_settings_color')}</label>
              <div className="flex gap-2">
                {LABEL_COLORS.map(c => (
                  <button
                    key={c}
                    onClick={() => setNewColor(c)}
                    className={`w-6 h-6 rounded-full transition-transform ${
                      newColor === c
                        ? 'scale-125 ring-2 ring-offset-1 ring-gray-400'
                        : 'hover:scale-110'
                    }`}
                    style={{ background: c }}
                  />
                ))}
              </div>
            </div>
            <div className="flex justify-end gap-2">
              <Button
                variant="ghost"
                size="sm"
                onClick={() => { setShowCreate(false); setNewName('') }}
              >
                {t('common_cancel')}
              </Button>
              <Button
                size="sm"
                onClick={() => createMut.mutate()}
                disabled={!newName.trim() || !selectedAccountId}
                loading={createMut.isPending}
              >
                {t('common_create')}
              </Button>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}

// ── Stub tabs ─────────────────────────────────────────────────────────────────

function FiltersTab() {
  const { t } = useTranslation('mail')
  const qc = useQueryClient()
  const filtersQ = useQuery({ queryKey: ['mail-filters'], queryFn: mailApi.listFilters })
  const labelsQ  = useQuery({ queryKey: ['mail-labels'],  queryFn: mailApi.listLabels })
  const labelName = (id: string | null) => id ? (labelsQ.data?.labels.find(l => l.id === id)?.name ?? '?') : null
  const delMut = useMutation({
    mutationFn: (id: string) => mailApi.deleteFilter(id),
    onSuccess:  () => qc.invalidateQueries({ queryKey: ['mail-filters'] }),
  })
  const filters = filtersQ.data ?? []

  const condText = (f: typeof filters[number]) => {
    const parts: string[] = []
    if (f.from_contains)    parts.push(`${t('mail_filter_from', { defaultValue: 'De' })}: ${f.from_contains}`)
    if (f.to_contains)      parts.push(`${t('mail_filter_to', { defaultValue: 'À' })}: ${f.to_contains}`)
    if (f.subject_contains) parts.push(`${t('subject', { defaultValue: 'Objet' })}: ${f.subject_contains}`)
    if (f.query_contains)   parts.push(`${t('mail_filter_has_words', { defaultValue: 'Contient' })}: ${f.query_contains}`)
    return parts.join(' · ') || '—'
  }
  const actChips = (f: typeof filters[number]) => {
    const c: string[] = []
    if (f.act_archive)   c.push(t('archive', { defaultValue: 'Archiver' }))
    if (f.act_mark_read) c.push(t('mail_mark_read', { defaultValue: 'Marquer lu' }))
    if (f.act_star)      c.push(t('folder_starred', { defaultValue: 'Suivre' }))
    if (f.act_important) c.push(t('folder_important', { defaultValue: 'Important' }))
    if (f.act_trash)     c.push(t('delete', { defaultValue: 'Corbeille' }))
    if (f.act_spam)      c.push(t('spam_report', { defaultValue: 'Spam' }))
    if (f.act_label_id)  c.push(`🏷 ${labelName(f.act_label_id)}`)
    return c
  }

  return (
    <div>
      <div className="flex items-center justify-between mb-4">
        <p className="text-sm text-text-secondary">{t('mail_settings_filters_count', { count: filters.length, defaultValue: `${filters.length} filtre(s)` })}</p>
      </div>
      <p className="text-xs text-text-tertiary mb-4">
        {t('filters_hint', { defaultValue: 'Créez un filtre depuis la barre de recherche (icône filtres) → « Créer un filtre ». Les filtres s\'appliquent aux nouveaux messages reçus.' })}
      </p>
      {filtersQ.isLoading ? (
        <div className="py-10 flex justify-center"><Loader2 className="animate-spin text-text-tertiary" /></div>
      ) : filters.length === 0 ? (
        <div className="py-12 text-center text-sm text-text-tertiary">{t('filters_empty', { defaultValue: 'Aucun filtre.' })}</div>
      ) : (
        <div className="divide-y divide-border/50 border border-border rounded-lg">
          {filters.map(f => (
            <div key={f.id} className="flex items-start gap-4 px-4 py-3">
              <div className="flex-1 min-w-0">
                <div className="text-sm text-text-primary">{condText(f)}</div>
                <div className="flex flex-wrap gap-1.5 mt-1.5">
                  {actChips(f).map((c, i) => (
                    <span key={i} className="text-xs bg-surface-1 border border-border rounded-full px-2 py-0.5 text-text-secondary">{c}</span>
                  ))}
                </div>
              </div>
              <button onClick={() => delMut.mutate(f.id)} title={t('delete', { defaultValue: 'Supprimer' })}
                className="p-1.5 rounded hover:bg-danger/10 hover:text-danger text-text-tertiary transition-colors flex-shrink-0">
                <Trash2 size={15} />
              </button>
            </div>
          ))}
        </div>
      )}

      <BlockedSendersSection />
    </div>
  )
}

// ── Adresses bloquées ───────────────────────────────────────────────────────────

function BlockedSendersSection() {
  const { t } = useTranslation('mail')
  const qc = useQueryClient()
  const [email, setEmail] = useState('')
  const blockedQ = useQuery({ queryKey: ['mail-blocked'], queryFn: mailApi.listBlocked })
  const blocked = blockedQ.data ?? []

  const addMut = useMutation({
    mutationFn: () => mailApi.blockSender(email.trim()),
    onSuccess:  () => { setEmail(''); qc.invalidateQueries({ queryKey: ['mail-blocked'] }) },
  })
  const delMut = useMutation({
    mutationFn: (id: string) => mailApi.unblockSender(id),
    onSuccess:  () => qc.invalidateQueries({ queryKey: ['mail-blocked'] }),
  })

  const canAdd = email.trim().includes('@')

  return (
    <div className="mt-10">
      <h3 className="text-sm font-semibold text-text-primary mb-1">
        {t('blocked_title', { defaultValue: 'Adresses bloquées' })}
      </h3>
      <p className="text-xs text-text-tertiary mb-4">
        {t('blocked_hint', { defaultValue: 'Les messages des adresses bloquées sont automatiquement déplacés vers le spam.' })}
      </p>

      <div className="flex items-center gap-2 mb-4">
        <input
          type="email"
          value={email}
          onChange={e => setEmail(e.target.value)}
          onKeyDown={e => { if (e.key === 'Enter' && canAdd) addMut.mutate() }}
          placeholder={t('blocked_placeholder', { defaultValue: 'adresse@exemple.com' })}
          className="flex-1 border border-border rounded-lg px-3 py-2 text-sm text-text-primary
                     focus:outline-none focus:ring-2 focus:ring-primary/30"
        />
        <Button size="sm" onClick={() => addMut.mutate()} disabled={!canAdd} loading={addMut.isPending}>
          {t('blocked_add', { defaultValue: 'Bloquer' })}
        </Button>
      </div>

      {blockedQ.isLoading ? (
        <div className="py-6 flex justify-center"><Loader2 className="animate-spin text-text-tertiary" /></div>
      ) : blocked.length === 0 ? (
        <div className="py-8 text-center text-sm text-text-tertiary">
          {t('blocked_empty', { defaultValue: 'Aucune adresse bloquée.' })}
        </div>
      ) : (
        <div className="divide-y divide-border/50 border border-border rounded-lg">
          {blocked.map(b => (
            <div key={b.id} className="flex items-center gap-3 px-4 py-2.5">
              <Ban size={15} className="text-text-tertiary flex-shrink-0" />
              <span className="flex-1 text-sm text-text-primary truncate">{b.email}</span>
              <button onClick={() => delMut.mutate(b.id)} title={t('blocked_remove', { defaultValue: 'Débloquer' })}
                className="text-xs font-medium text-primary hover:underline flex-shrink-0">
                {t('blocked_remove', { defaultValue: 'Débloquer' })}
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}

function ForwardingTab() {
  const { t } = useTranslation('mail')
  return (
    <div className="py-16 text-center">
      <p className="text-sm text-text-tertiary font-medium">{t('mail_settings_tab_forwarding')}</p>
      <p className="text-xs text-text-tertiary mt-1">
        {t('mail_settings_coming_soon')}
      </p>
    </div>
  )
}

// ── Anti-spam (Bayes) tab ──────────────────────────────────────────────────────

function SpamTab() {
  const { t } = useTranslation('mail')
  const qc = useQueryClient()
  const { data, isLoading } = useQuery({ queryKey: ['mail-spam-stats'], queryFn: mailApi.getSpamStats })

  const settingsMut = useMutation({
    mutationFn: (dto: { auto_classify?: boolean; threshold?: number }) => mailApi.updateSpamSettings(dto),
    onSuccess:  () => qc.invalidateQueries({ queryKey: ['mail-spam-stats'] }),
  })

  const [trained, setTrained] = useState<{ spam_messages: number; ham_messages: number; capped: boolean } | null>(null)
  const trainMut = useMutation({
    mutationFn: () => mailApi.trainSpam(),
    onSuccess:  (res) => { setTrained(res); qc.invalidateQueries({ queryKey: ['mail-spam-stats'] }) },
  })

  if (isLoading || !data) {
    return <div className="py-16 flex justify-center"><Loader2 size={20} className="animate-spin text-text-tertiary" /></div>
  }

  // Le seuil est exposé en niveaux compréhensibles plutôt qu'en probabilité brute.
  const thresholdOptions = [
    { value: '0.99', label: t('spam_threshold_strict',   { defaultValue: 'Strict (peu de faux positifs)' }) },
    { value: '0.95', label: t('spam_threshold_balanced', { defaultValue: 'Équilibré (recommandé)' }) },
    { value: '0.85', label: t('spam_threshold_aggressive', { defaultValue: 'Agressif (attrape plus)' }) },
  ]
  const currentThreshold = thresholdOptions.find(o => Math.abs(Number(o.value) - data.threshold) < 0.001)?.value ?? '0.95'

  return (
    <div>
      <SettingsRow
        label={t('spam_auto_classify', { defaultValue: 'Filtrage automatique' })}
        description={t('spam_auto_classify_desc', { defaultValue: 'Déplacer automatiquement vers le dossier Spam les messages reconnus comme indésirables par le modèle bayésien personnel.' })}
      >
        <label className="flex items-center gap-2 cursor-pointer">
          <input
            type="checkbox"
            checked={data.auto_classify}
            onChange={e => settingsMut.mutate({ auto_classify: e.target.checked })}
          />
          <span className="text-sm text-text-primary">{t('spam_auto_classify_on', { defaultValue: 'Activer le tri automatique' })}</span>
        </label>
      </SettingsRow>

      <SettingsRow
        label={t('spam_threshold', { defaultValue: 'Sensibilité' })}
        description={t('spam_threshold_desc', { defaultValue: 'Niveau de confiance requis avant de déplacer un message vers le Spam.' })}
      >
        <div className="max-w-xs">
          <Dropdown
            value={currentThreshold}
            onChange={v => settingsMut.mutate({ threshold: Number(v) })}
            options={thresholdOptions}
          />
        </div>
      </SettingsRow>

      <SettingsRow
        label={t('spam_model', { defaultValue: 'Modèle d\'apprentissage' })}
        description={t('spam_model_desc', { defaultValue: 'Le modèle apprend de vos actions « Spam » / « Pas un spam ». Vous pouvez le reconstruire à partir de vos messages actuels.' })}
      >
        <div className="space-y-3">
          <div className="flex gap-4 text-sm text-text-secondary">
            <span><strong className="text-text-primary">{data.spam_messages}</strong> {t('spam_examples', { defaultValue: 'exemples spam' })}</span>
            <span><strong className="text-text-primary">{data.ham_messages}</strong> {t('ham_examples', { defaultValue: 'exemples légitimes' })}</span>
            <span><strong className="text-text-primary">{data.distinct_tokens}</strong> {t('spam_tokens', { defaultValue: 'mots appris' })}</span>
          </div>
          <Button onClick={() => trainMut.mutate()} disabled={trainMut.isPending} variant="secondary"
            icon={trainMut.isPending ? <Loader2 size={14} className="animate-spin" /> : <RefreshCw size={14} />}>
            {t('spam_retrain', { defaultValue: 'Réentraîner le modèle' })}
          </Button>
          {trained && (
            <p className="text-xs text-emerald-600 flex items-center gap-1">
              <Check size={14} />
              {t('spam_retrain_done', {
                defaultValue: 'Modèle reconstruit : {{spam}} spams, {{ham}} légitimes.',
                spam: trained.spam_messages, ham: trained.ham_messages,
              })}
            </p>
          )}
          {data.spam_messages + data.ham_messages < 20 && (
            <p className="text-xs text-amber-600">
              {t('spam_need_more', { defaultValue: 'Le tri automatique s\'active après ~20 exemples. Continuez à marquer vos spams.' })}
            </p>
          )}
        </div>
      </SettingsRow>
    </div>
  )
}

// ── Main component ────────────────────────────────────────────────────────────

export default function MailSettingsPage() {
  const { t } = useTranslation('mail')
  const [activeTab, setActiveTab] = useState<TabId>('general')

  const TAB_LABELS: Record<TabId, string> = {
    general:    t('mail_settings_tab_general'),
    accounts:   t('mail_settings_tab_accounts'),
    labels:     t('mail_settings_tab_labels'),
    filters:    t('mail_settings_tab_filters'),
    spam:       t('mail_settings_tab_spam', { defaultValue: 'Anti-spam' }),
    forwarding: t('mail_settings_tab_forwarding'),
  }

  return (
    <div className="flex flex-col h-full bg-white overflow-hidden">
      {/* Breadcrumb header */}
      <div
        className="flex items-center gap-2 px-6 py-2.5 border-b border-[#e8eaed] flex-shrink-0"
        style={{ background: '#f8f9fa' }}
      >
        <Link
          to="/mail"
          className="flex items-center gap-1.5 text-sm text-[#1a73e8] hover:underline"
        >
          <ArrowLeft size={14} />
          {t('mail_settings_breadcrumb_mail')}
        </Link>
        <span className="text-text-tertiary text-sm">/</span>
        <div className="flex items-center gap-1.5">
          <Mail size={15} className="text-text-secondary" />
          <span className="text-sm text-text-primary">{t('mail_settings_breadcrumb_settings')}</span>
        </div>
      </div>

      {/* Tab bar (Gmail-style) */}
      <div
        className="flex items-end border-b border-[#e8eaed] px-4 flex-shrink-0 overflow-x-auto"
        style={{ background: '#fff' }}
      >
        {TAB_IDS.map(id => (
          <button
            key={id}
            onClick={() => setActiveTab(id)}
            className={`px-4 py-3 text-sm border-b-2 -mb-px transition-colors whitespace-nowrap ${
              activeTab === id
                ? 'border-[#1a73e8] text-[#1a73e8] font-medium'
                : 'border-transparent text-[#5f6368] hover:text-[#202124] hover:bg-[#f1f3f4]'
            }`}
          >
            {TAB_LABELS[id]}
          </button>
        ))}
      </div>

      {/* Settings content */}
      <div className="flex-1 overflow-y-auto">
        <div className="max-w-3xl mx-auto px-8 py-6">
          {activeTab === 'general'    && <GeneralTab />}
          {activeTab === 'accounts'   && <AccountsTab />}
          {activeTab === 'labels'     && <LabelsTab />}
          {activeTab === 'filters'    && <FiltersTab />}
          {activeTab === 'spam'       && <SpamTab />}
          {activeTab === 'forwarding' && <ForwardingTab />}
        </div>
      </div>
    </div>
  )
}
