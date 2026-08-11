import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import { useQuery } from '@tanstack/react-query'
import { ArrowLeft, Mail } from 'lucide-react'
import { mailApi } from './api'
import { GeneralTab } from './settings/GeneralTab'
import { AccountsTab } from './settings/AccountsTab'
import { LabelsTab } from './settings/LabelsTab'
import { FiltersTab } from './settings/FiltersTab'
import { SpamTab } from './settings/SpamTab'
import { ForwardingTab } from './settings/ForwardingTab'
import { MailboxAccessTab } from './settings/MailboxAccessTab'
import { EncryptionTab } from './settings/EncryptionTab'

// ── Types ─────────────────────────────────────────────────────────────────────

type TabId = 'general' | 'accounts' | 'labels' | 'filters' | 'spam' | 'encryption' | 'forwarding' | 'mailbox'

// ── Main component ────────────────────────────────────────────────────────────

export default function MailSettingsPage() {
  const { t } = useTranslation('mail')
  // Land directly on the accounts tab when returning from an OAuth flow. The
  // signatures section now lives inside the General tab, so `#signatures`
  // (still used by the composer's "Gérer les signatures" link) opens General
  // and scrolls to that section — handled in the effect below.
  const [activeTab, setActiveTab] = useState<TabId>(() =>
    window.location.hash === '#signatures' ? 'general'
      : window.location.search.includes('oauth=') ? 'accounts'
      : 'general'
  )

  // When arriving with the #signatures anchor, make sure the General tab is
  // shown and scroll the Signature section into view once it has rendered.
  useEffect(() => {
    if (window.location.hash !== '#signatures') return
    setActiveTab('general')
    // Defer to the next frame so the General tab (and its #signatures node) exist.
    const id = requestAnimationFrame(() =>
      document.getElementById('signatures')?.scrollIntoView({ behavior: 'smooth', block: 'start' })
    )
    return () => cancelAnimationFrame(id)
  }, [])

  // The "Chiffrement" tab only appears when the admin enabled OpenPGP.
  const { data: pgpStatus } = useQuery({ queryKey: ['mail-pgp-status'], queryFn: mailApi.pgpStatus })
  const gpgEnabled = !!pgpStatus?.enabled

  const TAB_IDS: TabId[] = [
    'general', 'accounts', 'labels', 'filters', 'spam',
    ...(gpgEnabled ? ['encryption' as const] : []),
    'forwarding', 'mailbox',
  ]

  const TAB_LABELS: Record<TabId, string> = {
    general:    t('mail_settings_tab_general'),
    accounts:   t('mail_settings_tab_accounts'),
    labels:     t('mail_settings_tab_labels'),
    filters:    t('mail_settings_tab_filters'),
    spam:       t('mail_settings_tab_spam', { defaultValue: 'Anti-spam' }),
    encryption: t('mail_settings_tab_encryption', { defaultValue: 'Chiffrement' }),
    forwarding: t('mail_settings_tab_forwarding'),
    mailbox:    t('mail_settings_tab_mailbox', { defaultValue: 'Accès client' }),
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

      {/* Tab bar */}
      <div
        className="flex items-end border-b border-[#e8eaed] px-4 flex-shrink-0 overflow-x-auto overflow-y-hidden"
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
        <div className="px-8 py-6">
          {activeTab === 'general'    && <GeneralTab />}
          {activeTab === 'accounts'   && <AccountsTab />}
          {activeTab === 'labels'     && <LabelsTab />}
          {activeTab === 'filters'    && <FiltersTab />}
          {activeTab === 'spam'       && <SpamTab />}
          {activeTab === 'encryption' && <EncryptionTab />}
          {activeTab === 'forwarding' && <ForwardingTab />}
          {activeTab === 'mailbox'    && <MailboxAccessTab />}
        </div>
      </div>
    </div>
  )
}
