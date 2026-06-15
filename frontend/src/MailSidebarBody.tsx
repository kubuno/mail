import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate, useLocation } from 'react-router-dom'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import {
  Inbox, Send, FileText, Star, ShieldAlert, Trash2,
  ChevronDown, ChevronRight, Plus, Tag, Circle, MailOpen,
  ShoppingBag, Users, Info, MessagesSquare, Settings2,
  Clock, Bookmark, CalendarClock, MailX, type LucideIcon,
} from 'lucide-react'
import { SidebarNavItem, prompt } from '@kubuno/sdk'
import { useMailStore } from './store'
import { mailApi } from './api'

// Dossiers principaux (toujours visibles)
const MAIN_FOLDERS = [
  { id: 'inbox',     key: 'folder_inbox',     label: 'Boîte de réception', icon: Inbox,    path: '/mail' },
  { id: 'starred',   key: 'folder_starred',   label: 'Messages suivis',    icon: Star,     path: '/mail/starred' },
  { id: 'snoozed',   key: 'folder_pending',   label: 'En attente',         icon: Clock,    path: '/mail/snoozed' },
  { id: 'important', key: 'folder_important', label: 'Important',          icon: Bookmark, path: '/mail/important' },
  { id: 'sent',      key: 'folder_sent',      label: 'Messages envoyés',   icon: Send,     path: '/mail/sent' },
  { id: 'drafts',    key: 'folder_drafts',    label: 'Brouillons',         icon: FileText, path: '/mail/drafts' },
] as const

// Catégories de la boîte de réception (classées côté client par expéditeur/sujet)
const CATEGORIES: { id: string; label: string; icon: LucideIcon }[] = [
  { id: 'purchases',     label: 'Achats',          icon: ShoppingBag },
  { id: 'social',        label: 'Réseaux sociaux', icon: Users },
  { id: 'notifications', label: 'Notifications',   icon: Info },
  { id: 'forums',        label: 'Forums',          icon: MessagesSquare },
  { id: 'promotions',    label: 'Promotions',      icon: Tag },
]

// Dossiers secondaires (derrière « Plus »)
const MORE_FOLDERS = [
  { id: 'scheduled',     key: 'folder_scheduled',     label: 'Planifié',              icon: CalendarClock, path: '/mail/scheduled' },
  { id: 'all',           key: 'folder_all',           label: 'Tous les messages',     icon: MailOpen,      path: '/mail/all' },
  { id: 'spam',          key: 'folder_spam',          label: 'Spam',                  icon: ShieldAlert,   path: '/mail/spam' },
  { id: 'trash',         key: 'folder_trash',         label: 'Corbeille',             icon: Trash2,        path: '/mail/trash' },
  { id: 'subscriptions', key: 'folder_subscriptions', label: 'Gérer les abonnements', icon: MailX,         path: '/mail/subscriptions' },
] as const

export default function MailSidebarBody({ collapsed = false }: { collapsed?: boolean }) {
  const { t } = useTranslation('mail')
  const navigate = useNavigate()
  const { pathname } = useLocation()
  const qc = useQueryClient()
  const [showMore,   setShowMore]   = useState(false)
  const [showLabels, setShowLabels] = useState(true)
  const { setComposeOpen, inboxCategory, setInboxCategory, accounts } = useMailStore()

  const { data: accountsData } = useQuery({ queryKey: ['mail-accounts'], queryFn: mailApi.listAccounts })
  const hasAccount = !!(accountsData?.accounts?.length)

  const { data: labelsData } = useQuery({
    queryKey: ['mail-labels'], queryFn: mailApi.listLabels, enabled: hasAccount,
  })
  const { data: counts } = useQuery({
    queryKey: ['mail-counts'], queryFn: mailApi.getCounts, enabled: hasAccount, refetchInterval: 60_000,
  })

  // Compteurs par catégorie : calculés côté client sur les fils non-lus de la boîte.
  const { data: inboxData } = useQuery({
    queryKey: ['mail-threads', 'inbox', null, null, false, ''],
    queryFn:  () => mailApi.listThreads({ folder: 'inbox', limit: 200 }),
    enabled:  hasAccount, refetchInterval: 60_000,
  })
  const catCounts = useMemo(() => {
    const m: Record<string, number> = {}
    for (const th of inboxData?.threads ?? []) {
      if (th.unread_count <= 0) continue
      const e = th.last_sender_email ?? ''
      const s = th.subject ?? ''
      const cat =
        /twitter|facebook|linkedin|instagram|tiktok|youtube|pinterest|snapchat|meta\.com|x\.com/i.test(e) ? 'social'
        : /forum|digest|groups?@|discourse|mailing.?list|listserv|community|googlegroups/i.test(e) ? 'forums'
        : /order|commande|re[çc]u|facture|invoice|shipping|livraison|delivery|tracking|colis|exp[ée]di|amazon|paypal|stripe|achat|purchase|payment|paiement|receipt/i.test(`${e} ${s}`) ? 'purchases'
        : /notification|alert|update|security|account|billing/i.test(e) ? 'notifications'
        : /no.?reply|newsletter|noreply|promo|marketing|info@|hello@|contact@|deals?@|offers?@/i.test(e) ? 'promotions'
        : 'principale'
      m[cat] = (m[cat] ?? 0) + 1
    }
    return m
  }, [inboxData])

  const labels = labelsData?.labels.filter(l => !l.is_system) ?? []
  const isInboxView = pathname === '/mail' || pathname === '/mail/'

  const num = (n?: number) => (n && n > 0 ? n : undefined)
  const badgeFor = (id: string): number | undefined => {
    switch (id) {
      case 'inbox':     return num(counts?.unread.inbox)
      case 'starred':   return num(counts?.starred)
      case 'snoozed':   return num(counts?.snoozed)
      case 'important': return num(counts?.important)
      case 'drafts':    return num(counts?.drafts)
      case 'spam':      return num(counts?.unread.spam)
      case 'scheduled': return num(counts?.scheduled)
      default:          return undefined
    }
  }

  // ── Créer un libellé ──────────────────────────────────────────────────────
  const onCreateLabel = async () => {
    const accountId = accounts.find(a => a.is_default)?.id ?? accounts[0]?.id ?? accountsData?.accounts?.[0]?.id
    if (!accountId) return
    const name = await prompt({
      title:        t('label_create', { defaultValue: 'Créer un libellé' }),
      placeholder:  t('label_name', { defaultValue: 'Nom du libellé' }),
      confirmLabel: t('common_create', { defaultValue: 'Créer' }),
    })
    if (!name?.trim()) return
    await mailApi.createLabel({ account_id: accountId, name: name.trim() }).catch(() => {})
    qc.invalidateQueries({ queryKey: ['mail-labels'] })
    qc.invalidateQueries({ queryKey: ['mail-counts'] })
  }

  // ── Item dossier ──────────────────────────────────────────────────────────
  const folderActive = (id: string, path: string) =>
    pathname === path || (id === 'inbox' && isInboxView && inboxCategory === 'principale')

  return (
    <>
      {/* Bouton Nouveau message */}
      {collapsed ? (
        <div className="flex justify-center mb-2">
          <button onClick={() => setComposeOpen(true)} title={t('new_message')}
            className="w-10 h-10 flex items-center justify-center bg-white rounded-full transition-shadow"
            style={{ boxShadow: '0 1px 3px rgba(60,64,67,0.3), 0 4px 8px rgba(60,64,67,0.15)' }}>
            <Plus size={20} className="text-text-secondary" />
          </button>
        </div>
      ) : (
        <div className="px-3 mb-2">
          <button onClick={() => setComposeOpen(true)}
            className="flex items-center gap-3 bg-white text-sm font-medium text-text-primary cursor-pointer w-full hover:shadow-md transition-shadow"
            style={{ padding: '14px 24px', border: '1px solid #e0e0e0', borderRadius: '16px', boxShadow: '0 1px 3px rgba(0,0,0,0.12)' }}>
            <Plus size={20} className="text-text-secondary flex-shrink-0" />
            {t('new_message')}
          </button>
        </div>
      )}

      <nav className={`flex-1 space-y-0.5 overflow-y-auto ${collapsed ? 'px-2' : 'px-0 pr-2'}`}>
        {/* Dossiers principaux */}
        {MAIN_FOLDERS.map(f => (
          <SidebarNavItem
            key={f.id} collapsed={collapsed}
            label={t(f.key, { defaultValue: f.label })}
            icon={<f.icon size={18} className="flex-shrink-0" />}
            active={folderActive(f.id, f.path)}
            onClick={() => navigate(f.path)}
            badge={badgeFor(f.id)}
          />
        ))}

        {/* Catégories de la boîte de réception */}
        {CATEGORIES.map(c => (
          <SidebarNavItem
            key={c.id} collapsed={collapsed}
            label={t('mail_tab_' + c.id, { defaultValue: c.label })}
            icon={<c.icon size={18} className="flex-shrink-0" />}
            active={isInboxView && inboxCategory === c.id}
            onClick={() => { setInboxCategory(c.id); navigate('/mail') }}
            badge={num(catCounts[c.id])}
          />
        ))}

        {/* Dossiers secondaires (repliés : tout en icônes) */}
        {(collapsed || showMore) && MORE_FOLDERS.map(f => (
          <SidebarNavItem
            key={f.id} collapsed={collapsed}
            label={t(f.key, { defaultValue: f.label })}
            icon={<f.icon size={18} className="flex-shrink-0" />}
            active={pathname === f.path}
            onClick={() => navigate(f.path)}
            badge={badgeFor(f.id)}
          />
        ))}

        {!collapsed && (
          <button onClick={() => setShowMore(v => !v)}
            className="w-full flex items-center gap-3 pl-4 pr-3 py-[5px] rounded-r-full text-sm text-text-secondary hover:bg-surface-2 transition-colors">
            {showMore ? <ChevronDown size={18} /> : <ChevronRight size={18} />}
            <span>{showMore ? t('mail_less') : t('mail_more')}</span>
          </button>
        )}

        {/* Libellés */}
        {!collapsed && (
          <div className="pt-2 pb-1">
            <div className="w-full flex items-center justify-between pl-4 pr-3 py-1 text-xs font-semibold text-text-secondary uppercase tracking-wide">
              <button onClick={() => setShowLabels(v => !v)} className="flex-1 text-left hover:text-text-primary transition-colors">
                {t('labels')}
              </button>
              <button onClick={onCreateLabel} title={t('label_create', { defaultValue: 'Créer un libellé' })}
                className="p-0.5 rounded hover:bg-surface-2 hover:text-text-primary transition-colors">
                <Plus size={14} />
              </button>
            </div>

            {showLabels && labels.map(label => (
              <SidebarNavItem
                key={label.id} collapsed={false}
                label={label.name}
                icon={<Circle size={11} className="flex-shrink-0" style={{ color: label.color ?? '#5f6368', fill: label.color ?? '#5f6368' }} />}
                active={pathname === `/mail/label/${label.id}`}
                onClick={() => navigate(`/mail/label/${label.id}`)}
                badge={num(counts?.labels[label.id])}
              />
            ))}

            <SidebarNavItem
              collapsed={false}
              label={t('label_manage', { defaultValue: 'Gérer les libellés' })}
              icon={<Settings2 size={16} className="flex-shrink-0" />}
              active={pathname === '/mail/settings'}
              onClick={() => navigate('/mail/settings')}
            />
            <SidebarNavItem
              collapsed={false}
              label={t('label_create', { defaultValue: 'Créer un libellé' })}
              icon={<Plus size={16} className="flex-shrink-0" />}
              active={false}
              onClick={onCreateLabel}
            />
          </div>
        )}
      </nav>
    </>
  )
}
