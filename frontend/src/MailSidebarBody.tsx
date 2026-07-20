import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate, useLocation } from 'react-router-dom'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import {
  Inbox, Send, FileText, Star, ShieldAlert, Trash2,
  ChevronDown, ChevronRight, Plus, Tag, Circle, MailOpen,
  Users, Info, Settings2,
  Clock, Bookmark, CalendarClock, MailX, type LucideIcon,
} from 'lucide-react'
import { SidebarNavItem, prompt } from '@kubuno/sdk'
import { useMailStore } from './store'
import { mailApi } from './api'
import { categoryTo } from './categoryRoute'

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
  { id: 'social',        label: 'Réseaux sociaux', icon: Users },
  { id: 'notifications', label: 'Notifications',   icon: Info },
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
  const { inboxCategory, setInboxCategory, accounts } = useMailStore()

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
      const cat =
        /twitter|facebook|linkedin|instagram|tiktok|youtube|pinterest|snapchat|meta\.com|x\.com/i.test(e) ? 'social'
        : /notification|alert|update|security|account|billing/i.test(e) ? 'notifications'
        : /no.?reply|newsletter|noreply|promo|marketing|info@|hello@|contact@|deals?@|offers?@/i.test(e) ? 'promotions'
        : 'main'
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
  // The inbox only lights up on the "main" category, otherwise it would
  // stay highlighted alongside the active category (both live on /mail).
  const folderActive = (id: string, path: string) =>
    id === 'inbox' ? isInboxView && inboxCategory === 'main' : pathname === path

  return (
    <>
      {/* The "New message" button lives in the shell's default New button now
          (NewActions: MailCreateMenu, registered in entry.ts). */}
      <nav className={`flex-1 overflow-y-auto py-1 space-y-0.5 ${collapsed ? 'px-2' : 'px-3'}`}>
        {/* Dossiers principaux */}
        {MAIN_FOLDERS.map(f => (
          <SidebarNavItem
            key={f.id} collapsed={collapsed}
            label={t(f.key, { defaultValue: f.label })}
            icon={<f.icon size={16} className="flex-shrink-0" />}
            active={folderActive(f.id, f.path)}
            // The inbox row IS the "main" category, hence its hash link.
            to={f.id === 'inbox' ? categoryTo('main') : f.path}
            badge={badgeFor(f.id)}
          />
        ))}

        {/* Catégories de la boîte de réception : filtres CLIENT sur /mail, donc
            pas de route propre mais un vrai lien de hash (/mail/#category/<id>),
            partageable et géré par l'historique. */}
        {CATEGORIES.map(c => (
          <SidebarNavItem
            key={c.id} collapsed={collapsed}
            label={t('mail_tab_' + c.id, { defaultValue: c.label })}
            icon={<c.icon size={16} className="flex-shrink-0" />}
            active={isInboxView && inboxCategory === c.id}
            to={categoryTo(c.id)}
            badge={num(catCounts[c.id])}
          />
        ))}

        {/* Dossiers secondaires (repliés : tout en icônes) */}
        {(collapsed || showMore) && MORE_FOLDERS.map(f => (
          <SidebarNavItem
            key={f.id} collapsed={collapsed}
            label={t(f.key, { defaultValue: f.label })}
            icon={<f.icon size={16} className="flex-shrink-0" />}
            active={pathname === f.path}
            to={f.path}
            badge={badgeFor(f.id)}
          />
        ))}

        {!collapsed && (
          <SidebarNavItem
            collapsed={false}
            label={showMore ? t('mail_less') : t('mail_more')}
            icon={showMore ? <ChevronDown size={16} className="flex-shrink-0" /> : <ChevronRight size={16} className="flex-shrink-0" />}
            active={false}
            onClick={() => setShowMore(v => !v)}
          />
        )}

        {/* Libellés */}
        {!collapsed && (
          <div className="pt-2 space-y-0.5">
            <div className="flex items-center gap-2 w-full px-3 py-1 text-[10px] font-bold text-text-tertiary uppercase tracking-widest">
              {/* Anchors (never <button>) like the rest of the left sidebar;
                  in-page actions so href="#". */}
              <a href="#" role="button" aria-expanded={showLabels}
                onClick={e => { e.preventDefault(); setShowLabels(v => !v) }}
                className="flex items-center gap-2 flex-1 min-w-0 cursor-pointer hover:text-text-secondary transition-colors">
                <ChevronDown className={`w-3 h-3 flex-shrink-0 transition-transform ${showLabels ? '' : '-rotate-90'}`} />
                <span className="truncate text-left">{t('labels')}</span>
              </a>
              <a href="#" role="button" title={t('label_create', { defaultValue: 'Créer un libellé' })}
                onClick={e => { e.preventDefault(); onCreateLabel() }}
                className="p-0.5 rounded cursor-pointer hover:bg-surface-2 hover:text-text-secondary transition-colors flex-shrink-0">
                <Plus size={14} />
              </a>
            </div>

            {showLabels && labels.map(label => (
              <SidebarNavItem
                key={label.id} collapsed={false}
                label={label.name}
                icon={<Circle size={11} className="flex-shrink-0" style={{ color: label.color ?? '#5f6368', fill: label.color ?? '#5f6368' }} />}
                active={pathname === `/mail/label/${label.id}`}
                to={`/mail/label/${label.id}`}
                badge={num(counts?.labels[label.id])}
              />
            ))}

            <SidebarNavItem
              collapsed={false}
              label={t('label_manage', { defaultValue: 'Gérer les libellés' })}
              icon={<Settings2 size={16} className="flex-shrink-0" />}
              active={pathname === '/mail/settings'}
              to="/mail/settings"
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
