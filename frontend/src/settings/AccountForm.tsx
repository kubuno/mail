import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { AlertCircle, KeyRound, Loader2, Wifi } from 'lucide-react'
import { mailApi, type CreateAccountDto, type EmailAccount } from '../api'
import { Button, Dropdown, Checkbox } from '@ui'
import { PROVIDERS, detectProvider } from './providers'
import { TextField, NumberField, PasswordField } from './formFields'
import { AccountTestReport, type StepResult, type TestResult } from './AccountTestReport'

// ── Account form ──────────────────────────────────────────────────────────────

export function AccountForm({ onClose, existing }: {
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

  // ── OAuth2 (Gmail / Microsoft) ──────────────────────────────────────────────
  // Which providers the server admin configured (never exposes secrets).
  const { data: oauthAvail } = useQuery({
    queryKey: ['mail-oauth-providers'],
    queryFn:  mailApi.oauthProviders,
    staleTime: 60_000,
  })
  const isOauthAccount = !!existing?.auth_kind && existing.auth_kind.startsWith('oauth')
  // A local mailbox is hosted by the instance: no external transport, no
  // password, no provider — editing exposes only the display name and default.
  const isLocal = existing?.kind === 'local'
  const [oauthLoading, setOauthLoading] = useState(false)
  // Manual (server/password) block folded away when the OAuth path is active;
  // a discreet link unfolds it for those who insist on an app password.
  const [showManual, setShowManual] = useState(false)

  function applyProviderAndProtocol(id: string, proto: 'imap' | 'pop3') {
    setProviderId(id)
    setProtocol(proto)
    setTestResult(null)
    setShowManual(false)
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
      // Pre-fill the usernames with the email address while they are still empty
      imap_username:     f.imap_username || f.email_address,
      smtp_username:     f.smtp_username || f.email_address,
      smtp_host:         p.smtp_host,
      smtp_port:         p.smtp_port,
      smtp_security:     p.smtp_security,
    }))
  }

  // When the email changes, refresh the usernames if a provider is selected
  // and the usernames still match the previous email value
  function handleEmailChange(email: string) {
    setForm(f => ({
      ...f,
      email_address: email,
      imap_username: (providerId !== 'custom' && (f.imap_username === f.email_address || !f.imap_username)) ? email : f.imap_username,
      smtp_username: (providerId !== 'custom' && (f.smtp_username === f.email_address || !f.smtp_username)) ? email : f.smtp_username,
    }))
  }

  const activeProvider = PROVIDERS.find(p => p.id === providerId)

  // OAuth provider relevant to the current selection (or the account itself).
  const oauthProviderId: 'google' | 'microsoft' | undefined =
    existing?.auth_kind === 'oauth_google'    ? 'google'
    : existing?.auth_kind === 'oauth_microsoft' ? 'microsoft'
    : activeProvider?.oauth
  const oauthProviderLabel = oauthProviderId === 'google' ? 'Google' : 'Microsoft'
  const oauthConfigured = oauthProviderId ? !!oauthAvail?.[oauthProviderId] : false
  // OAuth is the primary path for this selection: hide the whole manual block.
  const oauthActive = !!oauthProviderId && oauthConfigured
  // OAuth accounts never show servers/passwords (managed by the provider);
  // otherwise the manual block hides only while the OAuth path is active.
  const manualVisible = isLocal ? false : (isOauthAccount ? false : (!oauthActive || showManual))

  async function handleOauthSignin() {
    if (!oauthProviderId) return
    setOauthLoading(true)
    try {
      const { auth_url } = await mailApi.oauthStart(oauthProviderId)
      window.location.href = auth_url
    } catch {
      setOauthLoading(false)
    }
  }

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
      incoming_protocol: protocol,        // 'imap' | 'pop3' — tells the backend which test to run
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

  const setField = <K extends keyof CreateAccountDto>(key: K, value: CreateAccountDto[K]) =>
    setForm(f => ({ ...f, [key]: value }))

  // App-password providers (Gmail, Yahoo, iCloud…) DISPLAY the 16-char password in
  // space-separated groups ("xxxx xxxx xxxx xxxx") but REJECT it on IMAP/SMTP when
  // the spaces are pasted along. Strip whitespace for those providers ONLY — never
  // silently alter a password for other providers, where spaces may be legitimate.
  const normalizePassword = (v: string) =>
    activeProvider?.app_password ? v.replace(/\s+/g, '') : v

  return (
    <div className="fixed inset-0 bg-black/30 z-50 flex items-center justify-center p-4">
      <div className="bg-white rounded-2xl shadow-xl w-full max-w-lg max-h-[90vh] overflow-y-auto">
        <div className="px-6 py-4 border-b border-border">
          <h2 className="text-base font-semibold text-text-primary">
            {existing ? t('mail_settings_edit_account') : t('mail_settings_add_email_account')}
          </h2>
        </div>

        <div className="p-6 space-y-5">
          {isLocal ? (<>
            {/* Local mailbox: only the display name is editable; the address is
                fixed and assigned by the admin. */}
            <TextField
              label={t('mail_settings_display_name')}
              value={String(form.name ?? '')}
              onChange={v => setField('name', v)}
            />
            <div>
              <label className="block text-xs font-medium text-text-secondary mb-1.5">
                {t('mail_settings_email_address')}
              </label>
              <p className="text-sm text-text-primary">{existing?.email_address}</p>
              <p className="mt-1.5 text-xs text-text-tertiary">
                {t('mail_settings_hosted_form_hint', { defaultValue: 'Adresse hébergée par cette instance et attribuée par l’administrateur. Aucun réglage serveur n’est nécessaire.' })}
              </p>
            </div>
          </>) : (<>
          <div className="grid grid-cols-2 gap-4">
            <TextField
              label={t('mail_settings_display_name')}
              value={String(form.name ?? '')}
              onChange={v => setField('name', v)}
            />
            <TextField
              label={t('mail_settings_email_address')}
              type="email"
              value={form.email_address}
              onChange={handleEmailChange}
            />
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
            {activeProvider?.note && manualVisible && (
              <p className="mt-1.5 text-xs text-warning flex items-center gap-1">
                <AlertCircle size={11} className="flex-shrink-0" />
                {t(activeProvider.note)}
              </p>
            )}
          </div>

          {/* OAuth2 sign-in (Gmail / Microsoft) */}
          {oauthProviderId && (
            oauthConfigured ? (
              <div>
                <Button
                  className="w-full justify-center"
                  onClick={handleOauthSignin}
                  loading={oauthLoading}
                  icon={<KeyRound size={13} />}
                >
                  {isOauthAccount
                    ? t('mail_oauth_reconnect')
                    : t('mail_oauth_signin', { provider: oauthProviderLabel })}
                </Button>
                {isOauthAccount ? (
                  <p className="mt-2 text-xs text-text-tertiary">
                    {t('mail_oauth_managed', { provider: oauthProviderLabel })}
                  </p>
                ) : showManual ? (
                  <p className="mt-2 text-xs text-text-tertiary">
                    {t('mail_oauth_or_manual')}
                  </p>
                ) : (
                  <button
                    type="button"
                    onClick={() => setShowManual(true)}
                    className="mt-2 text-xs text-text-tertiary hover:text-text-secondary underline"
                  >
                    {t('mail_oauth_manual_link')}
                  </button>
                )}
              </div>
            ) : (
              <p className="text-xs text-text-tertiary">
                {isOauthAccount
                  ? t('mail_oauth_managed', { provider: oauthProviderLabel })
                  : t('mail_oauth_admin_hint', { provider: oauthProviderLabel })}
              </p>
            )
          )}

          {/* Manual configuration (servers + passwords) — hidden while the
              OAuth path is active, and always for OAuth-managed accounts */}
          {manualVisible && (<>
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
              <TextField
                className="col-span-2"
                label={t('mail_settings_server_field', { protocol: protocol.toUpperCase() })}
                value={form.imap_host}
                onChange={v => setField('imap_host', v)}
              />
              <NumberField
                label={t('mail_settings_port')}
                value={form.imap_port}
                onChange={v => setField('imap_port', v)}
              />
            </div>
            <div className="mt-3 grid grid-cols-2 gap-3">
              <div className="flex flex-col gap-1">
                <label className="text-xs font-medium text-text-secondary">{t('mail_settings_security')}</label>
                <Dropdown
                  className="w-full"
                  value={form.imap_security ?? ''}
                  onChange={v => setField('imap_security', v)}
                  options={[
                    { value: 'ssl',      label: 'SSL/TLS' },
                    { value: 'starttls', label: 'STARTTLS' },
                    { value: 'none',     label: t('mail_settings_security_none') },
                  ]}
                />
              </div>
              <TextField
                label={t('mail_settings_user_field', { protocol: protocol.toUpperCase() })}
                value={form.imap_username}
                onChange={v => setField('imap_username', v)}
              />
            </div>
            {!isOauthAccount && (
              <PasswordField
                className="mt-3"
                label={t('mail_settings_password_field', { protocol: protocol.toUpperCase() })}
                value={form.imap_password}
                onChange={v => setField('imap_password', normalizePassword(v))}
                placeholder={existing ? t('mail_settings_unchanged') : ''}
                show={showPassImap}
                onToggleShow={() => setShowPassImap(p => !p)}
              />
            )}
          </div>

          {/* SMTP */}
          <div>
            <h3 className="text-xs font-semibold text-text-tertiary uppercase tracking-wider mb-3">
              {t('mail_settings_outgoing_mail')}
            </h3>
            <div className="grid grid-cols-3 gap-3">
              <div className="col-span-2">
                <TextField
                  label={t('mail_settings_smtp_server')}
                  value={String(form.smtp_host ?? '')}
                  onChange={v => setField('smtp_host', v)}
                />
              </div>
              <NumberField
                label={t('mail_settings_port')}
                value={form.smtp_port}
                onChange={v => setField('smtp_port', v)}
              />
            </div>
            <div className="mt-3 grid grid-cols-2 gap-3">
              <div className="flex flex-col gap-1">
                <label className="text-xs font-medium text-text-secondary">{t('mail_settings_security')}</label>
                <Dropdown
                  className="w-full"
                  value={form.smtp_security ?? ''}
                  onChange={v => setField('smtp_security', v)}
                  options={[
                    { value: 'starttls', label: 'STARTTLS' },
                    { value: 'ssl',      label: 'SSL/TLS' },
                    { value: 'none',     label: t('mail_settings_security_none') },
                  ]}
                />
              </div>
              <TextField
                label={t('mail_settings_smtp_user')}
                value={String(form.smtp_username ?? '')}
                onChange={v => setField('smtp_username', v)}
              />
            </div>
            {!isOauthAccount && (
              <PasswordField
                className="mt-3"
                label={t('mail_settings_smtp_password')}
                value={form.smtp_password}
                onChange={v => setField('smtp_password', normalizePassword(v))}
                placeholder={existing ? t('mail_settings_unchanged') : ''}
                show={showPassSmtp}
                onToggleShow={() => setShowPassSmtp(p => !p)}
              />
            )}
          </div>
          </>)}
          </>)}

          <Checkbox
            label={t('mail_settings_default_account')}
            checked={!!form.is_default}
            onChange={v => setField('is_default', v)}
          />

          {testResult && <AccountTestReport result={testResult} appPasswordHint={!!activeProvider?.app_password} />}

          {mut.isError && (
            <div className="flex items-center gap-2 text-danger text-sm bg-danger/10 px-3 py-2 rounded-lg">
              <AlertCircle size={14} />
              {t('mail_settings_save_error')}
            </div>
          )}
        </div>

        <div className="px-6 py-4 border-t border-border flex items-center justify-between gap-3">
          {(manualVisible || isOauthAccount) ? (
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
          ) : <div />}
          <div className="flex items-center gap-3">
            <Button variant="ghost" onClick={onClose}>
              {t('common_cancel')}
            </Button>
            {/* Creating via the OAuth path happens through the sign-in button */}
            {!(!existing && oauthActive && !showManual) && (
              <Button onClick={() => mut.mutate()} loading={mut.isPending}>
                {existing ? t('common_save') : t('mail_settings_add')}
              </Button>
            )}
          </div>
        </div>
      </div>
    </div>
  )
}
