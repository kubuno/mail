import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate, useLocation } from 'react-router-dom'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import {
  Inbox, Send, FileText, Star, ShieldAlert, Trash2,
  ChevronDown, ChevronRight, Plus, Tag, MailOpen,
  Users, Info,
  Clock, Bookmark, CalendarClock, MailX, type LucideIcon,
} from 'lucide-react'
import { SidebarNavItem, useConfirm } from '@kubuno/sdk'
import { ColorPicker, ConfirmDialog } from '@ui'
import { useMailStore } from './store'
import { mailApi, type Label } from './api'
import { categoryTo } from './categoryRoute'
import NewLabelDialog, { type NewLabelResult } from './NewLabelDialog'
import MailLabelItem, { leafName } from './MailLabelItem'

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
  const [createOpen, setCreateOpen] = useState(false)
  const [creating,   setCreating]   = useState(false)
  const [editing,    setEditing]    = useState<Label | null>(null)
  const [subParent,  setSubParent]  = useState<Label | null>(null)
  const [colorFor,   setColorFor]   = useState<Label | null>(null)
  const { confirm, confirmState, handleConfirm, handleCancel } = useConfirm()
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

  const labels = labelsData?.labels?.filter(l => !l.is_system) ?? []
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
  const createAccountId =
    accounts.find(a => a.is_default)?.id ?? accounts[0]?.id ?? accountsData?.accounts?.[0]?.id

  const onCreateLabel = () => { if (createAccountId) setCreateOpen(true) }

  const onCreateLabelSubmit = async ({ name }: NewLabelResult) => {
    if (!createAccountId) return
    setCreating(true)
    await mailApi.createLabel({ account_id: createAccountId, name }).catch(() => {})
    setCreating(false)
    setCreateOpen(false)
    refreshLabels()
  }

  const refreshLabels = () => {
    qc.invalidateQueries({ queryKey: ['mail-labels'] })
    qc.invalidateQueries({ queryKey: ['mail-counts'] })
  }

  // "unread" labels only show up while they actually have unread threads;
  // "hide" keeps them out of the sidebar entirely (they stay in the settings).
  const visibleLabels = labels.filter(l => {
    const vis = l.list_visibility ?? 'show'
    if (vis === 'hide')   return pathname === `/mail/label/${l.id}`
    if (vis === 'unread') return (counts?.labels?.[l.id] ?? 0) > 0 || pathname === `/mail/label/${l.id}`
    return true
  })

  // ── Menu d'un libellé ─────────────────────────────────────────────────────
  const patchLabel = async (id: string, dto: Parameters<typeof mailApi.updateLabel>[1]) => {
    await mailApi.updateLabel(id, dto).catch(() => {})
    refreshLabels()
  }

  const onRenameSubmit = async ({ name }: NewLabelResult) => {
    if (!editing) return
    setCreating(true)
    // Renaming a parent renames its whole subtree, so "A/B" keeps following "A".
    const children = labels.filter(l => l.name.startsWith(`${editing.name}/`))
    await mailApi.updateLabel(editing.id, { name }).catch(() => {})
    for (const child of children) {
      await mailApi
        .updateLabel(child.id, { name: `${name}${child.name.slice(editing.name.length)}` })
        .catch(() => {})
    }
    setCreating(false)
    setEditing(null)
    refreshLabels()
  }

  const onDeleteLabel = async (label: Label) => {
    const ok = await confirm({
      title:       t('label_delete'),
      message:     t('label_delete_confirm', { name: leafName(label.name) }),
      confirmLabel: t('common_delete'),
      variant:      'danger',
    })
    if (!ok) return
    // Sub-labels are separate rows: drop them with their parent.
    for (const child of labels.filter(l => l.name.startsWith(`${label.name}/`))) {
      await mailApi.deleteLabel(child.id).catch(() => {})
    }
    await mailApi.deleteLabel(label.id).catch(() => {})
    refreshLabels()
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

            {showLabels && visibleLabels.map(label => (
              <MailLabelItem
                key={label.id}
                label={label}
                unread={num(counts?.labels?.[label.id])}
                active={pathname === `/mail/label/${label.id}`}
                to={`/mail/label/${label.id}`}
                actions={{
                  onSetColor:    color => patchLabel(label.id, { color }),
                  onPickCustom:  () => setColorFor(label),
                  onSetListVis:  v => patchLabel(label.id, { list_visibility: v }),
                  onSetMsgVis:   v => patchLabel(label.id, { message_list_visibility: v }),
                  onRename:      () => setEditing(label),
                  onDelete:      () => onDeleteLabel(label),
                  onAddSubLabel: () => setSubParent(label),
                }}
              />
            ))}

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

      {createOpen && (
        <NewLabelDialog
          labels={labels}
          pending={creating}
          onCancel={() => setCreateOpen(false)}
          onCreate={onCreateLabelSubmit}
        />
      )}

      {subParent && (
        <NewLabelDialog
          labels={labels}
          pending={creating}
          initialParent={subParent.name}
          onCancel={() => setSubParent(null)}
          onCreate={async result => { await onCreateLabelSubmit(result); setSubParent(null) }}
        />
      )}

      {editing && (
        <NewLabelDialog
          labels={labels}
          pending={creating}
          title={t('label_edit_title')}
          submitLabel={t('common_save')}
          initialName={leafName(editing.name)}
          initialParent={parentOf(editing.name)}
          excludeName={editing.name}
          onCancel={() => setEditing(null)}
          onCreate={onRenameSubmit}
        />
      )}

      {colorFor && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/30"
             onClick={() => setColorFor(null)}>
          <div onClick={e => e.stopPropagation()}>
            <ColorPicker
              t={t}
              color={colorFor.color ?? '#1a73e8'}
              onChange={() => {}}
              onClose={() => setColorFor(null)}
              onCancel={() => setColorFor(null)}
              onConfirm={hex => { patchLabel(colorFor.id, { color: hex }); setColorFor(null) }}
            />
          </div>
        </div>
      )}

      {confirmState && (
        <ConfirmDialog {...confirmState} onConfirm={handleConfirm} onCancel={handleCancel} />
      )}
    </>
  )
}

/** "A/B/C" → "A/B" (empty when the label is top-level). */
function parentOf(name: string) {
  const i = name.lastIndexOf('/')
  return i < 0 ? '' : name.slice(0, i)
}
