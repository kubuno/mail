import { useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate, useLocation } from 'react-router-dom'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import {
  Inbox, Send, FileText, Star, ShieldAlert, Trash2,
  ChevronDown, ChevronRight, Plus, Tag, MailOpen,
  Users, Info,
  Clock, Bookmark, CalendarClock, MailX, Folder, type LucideIcon,
} from 'lucide-react'
import { SidebarNavItem, useConfirm } from '@kubuno/sdk'
import { ColorPicker, ConfirmDialog } from '@ui'
import { useMailStore } from './store'
import { mailApi, type Label } from './api'
import { categoryTo } from './categoryRoute'
import NewLabelDialog, { type NewLabelResult } from './NewLabelDialog'
import MailLabelItem, { leafName } from './MailLabelItem'
import { isThreadDrag, readDraggedThreads, setThreadDropCaption } from './threadDnd'

// Dossiers principaux (toujours visibles)
const MAIN_FOLDERS = [
  { id: 'inbox',     key: 'folder_inbox',     label: 'Boîte de réception', icon: Inbox,    path: '/mail' },
  { id: 'starred',   key: 'folder_starred',   label: 'Messages suivis',    icon: Star,     path: '/mail/starred' },
  { id: 'snoozed',   key: 'folder_pending',   label: 'En attente',         icon: Clock,    path: '/mail/snoozed' },
  { id: 'important', key: 'folder_important', label: 'Important',          icon: Bookmark, path: '/mail/important' },
  { id: 'sent',      key: 'folder_sent',      label: 'Messages envoyés',   icon: Send,     path: '/mail/sent' },
  { id: 'drafts',    key: 'folder_drafts',    label: 'Brouillons',         icon: FileText, path: '/mail/drafts' },
] as const

// Catégories de la boîte de réception (assignées côté serveur à l'enregistrement)
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

// « Plus / Moins » stays where the user left it across a page refresh — a
// collapsed/expanded sidebar is a deliberate choice, not something to reset on F5.
const MORE_KEY = 'kubuno_mail_sidebar_more'
function initShowMore(): boolean {
  try { return localStorage.getItem(MORE_KEY) === '1' } catch { return false }
}

export default function MailSidebarBody({ collapsed = false }: { collapsed?: boolean }) {
  const { t } = useTranslation('mail')
  const navigate = useNavigate()
  const { pathname } = useLocation()
  const qc = useQueryClient()
  const [showMore,   setShowMore]   = useState(initShowMore)
  const [showLabels, setShowLabels] = useState(true)
  const [createOpen, setCreateOpen] = useState(false)
  const [creating,   setCreating]   = useState(false)
  const [editing,    setEditing]    = useState<Label | null>(null)
  const [subParent,  setSubParent]  = useState<Label | null>(null)
  const [colorFor,   setColorFor]   = useState<Label | null>(null)
  const [dropFolder, setDropFolder] = useState<string | null>(null)
  const { confirm, confirmState, handleConfirm, handleCancel } = useConfirm()
  const { inboxCategory, setInboxCategory, accounts, currentImapFolder, setSelectedThread, createLabelNonce } = useMailStore()

  // Clicking any folder / category / label must ALWAYS return to its list, even
  // when a conversation is open AND even when that folder is already the active
  // one. The open conversation lives in the store (mirrored to the URL hash via
  // replaceState, which the router does not see), so re-clicking the active row
  // is a router no-op that would otherwise leave the reader open. Clearing the
  // selection here runs on the click itself, whatever the router decides.
  const showList = () => setSelectedThread(null)

  // The account's own IMAP folders. Cheap query, refreshed like the counts.
  const { data: customFolders = [] } = useQuery({
    queryKey: ['mail-custom-folders'],
    queryFn:  mailApi.listCustomFolders,
    staleTime: 60_000,
  })

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
      // Category is stored with the message (services::categorize) — nothing to
      // classify here any more.
      m[th.category ?? 'main'] = (m[th.category ?? 'main'] ?? 0) + 1
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

  // The shell's New menu (« Nouveau libellé ») lives in a separate component; it
  // bumps a store nonce we watch here to open the create-label dialog.
  const prevLabelNonce = useRef(createLabelNonce)
  useEffect(() => {
    if (createLabelNonce !== prevLabelNonce.current) {
      prevLabelNonce.current = createLabelNonce
      onCreateLabel()
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [createLabelNonce])

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

  /** Folders that accept a dropped conversation, and what the drop does. */
  const DROP_FOLDERS: Record<string, (threadId: string) => Promise<unknown>> = {
    inbox:     id => mailApi.moveThread(id, 'inbox'),
    starred:   id => mailApi.starThread(id),
    important: id => mailApi.importantThread(id),
    spam:      id => mailApi.moveThread(id, 'spam'),
    trash:     id => mailApi.moveThread(id, 'trash'),
    all:       id => mailApi.moveThread(id, 'archive'),
  }

  const dropProps = (folderId: string, name: string) =>
    DROP_FOLDERS[folderId]
      ? {
          onDragOver: (e: React.DragEvent) => {
            if (!isThreadDrag(e)) return
            e.preventDefault()
            e.dataTransfer.dropEffect = 'move'
            setDropFolder(folderId)
            setThreadDropCaption(name)
          },
          onDragLeave: () => { setDropFolder(null); setThreadDropCaption(null) },
          onDrop: async (e: React.DragEvent) => {
            const ids = readDraggedThreads(e)
            setDropFolder(null)
            setThreadDropCaption(null)
            if (!ids.length) return
            e.preventDefault()
            for (const id of ids) await DROP_FOLDERS[folderId](id).catch(() => {})
            qc.invalidateQueries({ queryKey: ['mail-threads'] })
            qc.invalidateQueries({ queryKey: ['mail-counts'] })
          },
        }
      : {}

  // ── Item dossier ──────────────────────────────────────────────────────────
  // The inbox only lights up on the "main" category, otherwise it would
  // stay highlighted alongside the active category (both live on /mail).
  const folderActive = (id: string, path: string) =>
    id === 'inbox' ? isInboxView && inboxCategory === 'main' : pathname === path

  return (
    <>
      {/* The "New message" button lives in the shell's default New button now
          (MenuItem[] contributed to 'shell.new-actions' in entry.ts). */}
      <nav className={`flex-1 overflow-y-auto py-1 space-y-0.5 px-2`}>
        {/* Dossiers principaux */}
        {MAIN_FOLDERS.map(f => (
          <div key={f.id} {...dropProps(f.id, t(f.key, { defaultValue: f.label }))}
            className={`rounded-full ${dropFolder === f.id ? 'bg-[#fef7e0]' : ''}`}>
            <SidebarNavItem
              collapsed={collapsed}
              label={t(f.key, { defaultValue: f.label })}
              icon={<f.icon size={16} className="flex-shrink-0" />}
              active={folderActive(f.id, f.path)}
              // The inbox row IS the "main" category, hence its hash link.
              to={f.id === 'inbox' ? categoryTo('main') : f.path}
              onClick={showList}
              badge={badgeFor(f.id)}
            />
          </div>
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
            onClick={showList}
            badge={num(catCounts[c.id])}
          />
        ))}

        {/* Dossiers secondaires (repliés : tout en icônes) */}
        {(collapsed || showMore) && MORE_FOLDERS.map(f => (
          <div key={f.id} {...dropProps(f.id, t(f.key, { defaultValue: f.label }))}
            className={`rounded-full ${dropFolder === f.id ? 'bg-[#fef7e0]' : ''}`}>
            <SidebarNavItem
              collapsed={collapsed}
              label={t(f.key, { defaultValue: f.label })}
              icon={<f.icon size={16} className="flex-shrink-0" />}
              active={pathname === f.path}
              to={f.path}
              onClick={showList}
              badge={badgeFor(f.id)}
            />
          </div>
        ))}

        {!collapsed && (
          <SidebarNavItem
            collapsed={false}
            label={showMore ? t('mail_less') : t('mail_more')}
            icon={showMore ? <ChevronDown size={16} className="flex-shrink-0" /> : <ChevronRight size={16} className="flex-shrink-0" />}
            active={false}
            onClick={() => setShowMore(v => {
              const next = !v
              try { localStorage.setItem(MORE_KEY, next ? '1' : '0') } catch { /* ignore */ }
              return next
            })}
          />
        )}

        {/* The account's own provider folders, synced like the system ones.
            Only shown when there are any — most accounts have none. */}
        {!collapsed && customFolders.length > 0 && (
          <div className="pt-2 space-y-0.5">
            <div className="px-3 py-1 text-sm font-bold text-text-secondary">
              {t('mail_folders', { defaultValue: 'Dossiers' })}
            </div>
            {customFolders.map(f => (
              <SidebarNavItem
                key={f.name}
                collapsed={false}
                label={f.display || leafName(f.name)}
                icon={<Folder size={16} className="flex-shrink-0" />}
                active={currentImapFolder === f.name}
                to={`/mail/folder/${encodeURIComponent(f.name)}`}
                onClick={showList}
                badge={f.unread || undefined}
              />
            ))}
          </div>
        )}

        {/* Libellés */}
        {!collapsed && (
          <div className="pt-2 space-y-0.5">
            {/* Section title: plain 14px bold in the shell font. No small caps,
                no letter-spacing — they read as shouting at this size. */}
            <div className="flex items-center gap-2 w-full px-3 py-1 text-sm font-bold text-text-secondary">
              {/* Anchors (never <button>) like the rest of the left sidebar;
                  in-page actions so href="#". */}
              <a href="#" role="button" aria-expanded={showLabels}
                onClick={e => { e.preventDefault(); setShowLabels(v => !v) }}
                className="flex items-center gap-2 flex-1 min-w-0 cursor-pointer hover:text-text-primary transition-colors">
                <ChevronDown className={`w-3.5 h-3.5 flex-shrink-0 transition-transform ${showLabels ? '' : '-rotate-90'}`} />
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
                onClick={showList}
                actions={{
                  onSetColor:    color => patchLabel(label.id, { color }),
                  onPickCustom:  () => setColorFor(label),
                  onSetListVis:  v => patchLabel(label.id, { list_visibility: v }),
                  onSetMsgVis:   v => patchLabel(label.id, { message_list_visibility: v }),
                  onRename:      () => setEditing(label),
                  onDelete:      () => onDeleteLabel(label),
                  onAddSubLabel: () => setSubParent(label),
                  // Dropping a conversation here labels it and archives it,
                  // exactly like the menu's "Move to".
                  onDropThread: async threadIds => {
                    for (const id of threadIds) {
                      await mailApi.addLabel(id, label.id).catch(() => {})
                      await mailApi.moveThread(id, 'archive').catch(() => {})
                    }
                    qc.invalidateQueries({ queryKey: ['mail-threads'] })
                    refreshLabels()
                  },
                }}
              />
            ))}
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
