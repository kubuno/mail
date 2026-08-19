import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
  Reply, ReplyAll, Forward, Archive, Trash2, MailOpen, Mail as MailIcon,
  Clock, CheckSquare, FolderInput, Tag, BellOff, Search, ExternalLink,
} from 'lucide-react'
import { MenuDropdown, type MenuItem, type MenuDropdownPos } from '@ui'
import { ModuleServiceRegistry } from '@kubuno/sdk'
import type { Label, Thread } from './api'

export interface ThreadMenuActions {
  onReply:       () => void
  onReplyAll:    () => void
  onForward:     () => void
  onArchive:     () => void
  onDelete:      () => void
  onToggleRead:  () => void
  onSnooze:      () => void
  onAddToTasks:  () => void
  onMoveFolder:  (folder: 'spam' | 'trash') => void
  onMoveLabel:   (label: Label) => void
  onToggleLabel: (label: Label) => void
  onCreateLabel: () => void
  onMute:        () => void
  onSearchFrom:  () => void
  onOpenWindow:  () => void
}

/**
 * Right-click menu on a conversation row, following Gmail's layout: replies,
 * then row actions, then the two label submenus, then search/open-out.
 */
export default function ThreadContextMenu({
  thread, labels, pos, onClose, actions,
}: {
  thread:  Thread
  labels:  Label[]
  pos:     MenuDropdownPos
  onClose: () => void
  actions: ThreadMenuActions
}) {
  const { t } = useTranslation('mail')
  const unread = thread.unread_count > 0
  const sender = thread.last_sender_email

  // Only offered while the tasks module is installed and publishes its API —
  // modules must never assume a neighbour is there.
  const hasTasks = !!ModuleServiceRegistry.get('tasks', 'createTask')

  const attached = new Set((thread.labels ?? []).map(l => l.id))

  const items: MenuItem[] = [
    { type: 'action', icon: <Reply size={15} />,    label: t('mail_reply'),     onClick: actions.onReply },
    { type: 'action', icon: <ReplyAll size={15} />, label: t('mail_reply_all'), onClick: actions.onReplyAll },
    { type: 'action', icon: <Forward size={15} />,  label: t('mail_forward'),   onClick: actions.onForward },
    { type: 'separator' },
    { type: 'action', icon: <Archive size={15} />, label: t('archive', { defaultValue: 'Archiver' }), onClick: actions.onArchive },
    { type: 'action', icon: <Trash2 size={15} />,  label: t('delete'), shortcut: 'Suppr', onClick: actions.onDelete },
    {
      type: 'action',
      icon: unread ? <MailOpen size={15} /> : <MailIcon size={15} />,
      label: t(unread ? 'mail_mark_read' : 'mail_mark_unread', {
        defaultValue: unread ? 'Marquer comme lu' : 'Marquer comme non lu',
      }),
      onClick: actions.onToggleRead,
    },
    { type: 'action', icon: <Clock size={15} />, label: t('mail_snooze_action'), onClick: actions.onSnooze },
    ...(hasTasks
      ? [{
          type: 'action' as const, icon: <CheckSquare size={15} />,
          label: t('mail_add_to_tasks'),
          onClick: actions.onAddToTasks,
        }]
      : []),
    { type: 'separator' },
    {
      type:  'submenu',
      icon:  <FolderInput size={15} />,
      label: t('move_to', { defaultValue: 'Déplacer vers' }),
      items: [
        {
          type: 'custom',
          render: close => (
            <LabelPicker
              labels={labels}
              placeholder={t('move_to', { defaultValue: 'Déplacer vers' })}
              onPick={label => { actions.onMoveLabel(label); close() }}
            />
          ),
        },
        { type: 'separator' },
        { type: 'action', label: t('folder_spam',  { defaultValue: 'Spam' }),      onClick: () => actions.onMoveFolder('spam') },
        { type: 'action', label: t('folder_trash', { defaultValue: 'Corbeille' }), onClick: () => actions.onMoveFolder('trash') },
        { type: 'separator' },
        { type: 'action', label: t('common_create', { defaultValue: 'Créer' }), onClick: actions.onCreateLabel },
      ],
    },
    {
      type:  'submenu',
      icon:  <Tag size={15} />,
      label: t('mail_add_label'),
      items: [
        {
          type: 'custom',
          render: close => (
            <LabelPicker
              labels={labels}
              checkedIds={attached}
              placeholder={t('mail_add_label')}
              onPick={label => { actions.onToggleLabel(label); close() }}
            />
          ),
        },
        { type: 'separator' },
        { type: 'action', label: t('common_create', { defaultValue: 'Créer' }), onClick: actions.onCreateLabel },
      ],
    },
    { type: 'action', icon: <BellOff size={15} />, label: t('mail_mute'), onClick: actions.onMute },
    { type: 'separator' },
    {
      type: 'action', icon: <Search size={15} />,
      label: t('mail_search_from', { sender }),
      onClick: actions.onSearchFrom,
    },
    { type: 'separator' },
    {
      type: 'action', icon: <ExternalLink size={15} />,
      label: t('mail_open_new_window'),
      onClick: actions.onOpenWindow,
    },
  ]

  return <MenuDropdown items={items} pos={pos} onClose={onClose} minWidth={260} />
}

/** Searchable label list used by both submenus. */
function LabelPicker({
  labels, onPick, placeholder, checkedIds,
}: {
  labels:      Label[]
  onPick:      (label: Label) => void
  placeholder: string
  checkedIds?: Set<string>
}) {
  const [q, setQ] = useState('')
  const needle  = q.trim().toLowerCase()
  const matches = labels.filter(l => !needle || l.name.toLowerCase().includes(needle))

  return (
    <div className="px-2 pt-1 pb-2 w-[240px]">
      <div className="relative mb-1">
        <input
          autoFocus
          value={q}
          onChange={e => setQ(e.target.value)}
          placeholder={placeholder}
          className="w-full h-8 pl-2 pr-7 text-sm bg-transparent border-b border-border
                     focus:outline-none focus:border-primary text-text-primary"
        />
        <Search size={14} className="absolute right-1 top-2 text-text-tertiary pointer-events-none" />
      </div>
      <div className="max-h-56 overflow-y-auto">
        {matches.length === 0 ? (
          <div className="px-2 py-2 text-xs text-text-tertiary">—</div>
        ) : matches.map(l => (
          <a
            key={l.id}
            href="#"
            role="button"
            onClick={e => { e.preventDefault(); onPick(l) }}
            className="flex items-center gap-2 px-2 py-1.5 rounded text-sm text-text-primary
                       no-underline cursor-pointer hover:bg-surface-2"
          >
            {checkedIds && (
              <span className="w-3 text-primary">{checkedIds.has(l.id) ? '✓' : ''}</span>
            )}
            <span className="truncate">{l.name}</span>
          </a>
        ))}
      </div>
    </div>
  )
}
