import { useTranslation } from 'react-i18next'
import { CheckCircle2, XCircle, Info } from 'lucide-react'

// ── Connection test results ───────────────────────────────────────────────────

export interface StepResult { ok: boolean; error: string | null }

export interface TestResult {
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

/** Per-step report (connection + authentication) for the incoming and SMTP servers. */
export function AccountTestReport({ result, appPasswordHint = false }: {
  result:          TestResult
  /** When the selected provider uses app passwords, surface a hint on auth failure. */
  appPasswordHint?: boolean
}) {
  const { t } = useTranslation('mail')
  const proto = result.incoming.protocol.toUpperCase()
  const authFailed = !result.incoming.auth.ok || !result.smtp.auth.ok

  const rows: { label: string; step: StepResult; successText: string }[] = [
    { label: `${proto} — ${t('mail_settings_test_connection')}`,     step: result.incoming.connection, successText: t('mail_settings_test_server_reachable') },
    { label: `${proto} — ${t('mail_settings_test_auth')}`,           step: result.incoming.auth,       successText: t('mail_settings_test_creds_valid') },
    { label: `SMTP — ${t('mail_settings_test_connection')}`,         step: result.smtp.connection,     successText: t('mail_settings_test_server_reachable') },
    { label: `SMTP — ${t('mail_settings_test_auth')}`,               step: result.smtp.auth,           successText: t('mail_settings_test_creds_valid') },
  ]

  return (
    <div className="rounded-lg border border-border overflow-hidden text-sm">
      {appPasswordHint && authFailed && (
        <div className="flex items-start gap-2.5 px-3 py-2.5 border-b border-border bg-warning/10 text-warning">
          <Info size={15} className="flex-shrink-0 mt-px" />
          <span className="text-xs">{t('mail_settings_test_app_password_hint')}</span>
        </div>
      )}
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
}
