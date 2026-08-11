import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
  ChevronLeft, ChevronRight, MoreVertical, Archive, Trash2, Mail as MailIcon,
  ShieldAlert, ShieldCheck, FolderInput, Bookmark, BellOff, Clock,
  Printer, ExternalLink,
} from 'lucide-react'
import { MenuDropdown, type MenuItem as UiMenuItem, type MenuDropdownPos } from '@ui'
import { Thread } from '../api'
import type { SnoozePreset } from './threadActions'

export default function ThreadReaderToolbar({
  isMobile, thread, messageCount, currentFolder,
  onBack, moveTo, onDelete, onSpam, onThreadUnread, onImportant, onMute, onSnooze,
  snoozePresets, goRelative,
}: {
  isMobile:      boolean
  thread:        Thread
  messageCount:  number
  currentFolder: string
  onBack:        () => void
  moveTo:        (folder: string) => void
  onDelete:      () => void
  onSpam:        (folder: 'spam' | 'inbox') => void
  onThreadUnread: () => void
  onImportant:   () => void
  onMute:        () => void
  onSnooze:      (until: string) => void
  snoozePresets: () => SnoozePreset[]
  goRelative:    (dir: 1 | -1) => void
}) {
  const { t } = useTranslation('mail')
  // Mobile "⋮" menu (all the secondary actions live there, not in the toolbar).
  const [morePos, setMorePos] = useState<MenuDropdownPos | null>(null)
  const [moveOpen, setMoveOpen] = useState(false)

  // Secondary actions, shown in the mobile "⋮" menu (bottom sheet on touch,
  // with drill-in submenus for snooze and move).
  const moreItems: UiMenuItem[] = [
    { type: 'action', icon: <Bookmark size={15} />,
      label: t('folder_important', { defaultValue: 'Important' }),
      onClick: onImportant },
    { type: 'action', icon: <BellOff size={15} />,
      label: t('mail_mute'), onClick: onMute },
    { type: 'submenu', icon: <Clock size={15} />, label: t('mail_snooze_action'),
      items: snoozePresets().map(p => ({
        type: 'action' as const, label: p.label,
        onClick: () => onSnooze(p.until) })) },
    { type: 'separator' },
    { type: 'action', icon: <Bookmark size={15} />, label: t('folder_important', { defaultValue: 'Important' }),
      checked: !!thread.is_important, onClick: onImportant },
    currentFolder === 'spam'
      ? { type: 'action', icon: <ShieldCheck size={15} />, label: t('not_spam', { defaultValue: 'Pas un spam' }),
          onClick: () => onSpam('inbox') }
      : { type: 'action', icon: <ShieldAlert size={15} />, label: t('spam_report'),
          onClick: () => onSpam('spam') },
    { type: 'action', icon: <BellOff size={15} />, label: t('mute', { defaultValue: 'Ignorer la conversation' }),
      onClick: onMute },
    { type: 'submenu', icon: <Clock size={15} />, label: t('snooze', { defaultValue: 'Différer' }),
      items: snoozePresets().map(p => ({ type: 'action' as const, label: p.label, onClick: () => onSnooze(p.until) })) },
    { type: 'submenu', icon: <FolderInput size={15} />, label: t('move_to'),
      items: ([['inbox', t('folder_inbox')], ['archive', t('archive')], ['spam', t('folder_spam')], ['trash', t('folder_trash')]] as [string, string][])
        .filter(([f]) => f !== currentFolder)
        .map(([f, label]) => ({ type: 'action' as const, label, onClick: () => moveTo(f) })) },
    { type: 'separator' },
    { type: 'action', icon: <Printer size={15} />, label: t('print'), onClick: () => window.print() },
    { type: 'action', icon: <ExternalLink size={15} />, label: t('mail_open_new_window'),
      onClick: () => window.open(`${window.location.origin}/mail?thread=${thread.id}`, '_blank', 'noopener') },
  ]

  const TBtn = ({ onClick, title, children, danger }: {
    onClick?: () => void; title: string; children: React.ReactNode; danger?: boolean
  }) => (
    <button
      onClick={onClick}
      title={title}
      className={`p-2 rounded-full transition-colors
        ${danger
          ? 'hover:bg-danger/10 hover:text-danger text-text-secondary'
          : 'hover:bg-[#f1f3f4] text-[#444746]'}`}
    >
      {children}
    </button>
  )

  // ══ Toolbar ══════════════════════════════════════════════════════════
  // Mobile: back + the 2 primary actions + a "⋮" menu holding everything
  // else — never several wrapped rows of icons. Desktop: full bar.
  return isMobile ? (
    <div className="flex items-center justify-between pl-1 pr-2 h-12 border-b border-[#e0e0e0] flex-shrink-0 no-print">
      <TBtn onClick={onBack} title={t('back')}>
        <ChevronLeft size={22} />
      </TBtn>
      <div className="flex items-center gap-1">
        <TBtn title={t('archive')} onClick={() => moveTo('archive')}><Archive size={19} /></TBtn>
        <TBtn title={t('delete')} danger onClick={onDelete}><Trash2 size={19} /></TBtn>
        <button
          title={t('more_options')}
          onClick={e => { const r = e.currentTarget.getBoundingClientRect(); setMorePos(p => p ? null : { top: r.bottom + 4, left: Math.max(8, r.right - 260), minWidth: 260 }) }}
          className="p-2 rounded-full hover:bg-[#f1f3f4] text-[#444746] transition-colors">
          <MoreVertical size={19} />
        </button>
      </div>
      {morePos && <MenuDropdown pos={morePos} onClose={() => setMorePos(null)} items={moreItems} />}
    </div>
  ) : (
  <div className="flex items-center flex-wrap gap-0.5 px-2 min-h-[48px] py-1 border-b border-[#e0e0e0] flex-shrink-0 no-print">

    {/* Back to the list */}
    <TBtn onClick={onBack} title={t('back')}>
      <ChevronLeft size={20} />
    </TBtn>

    <div className="w-px h-5 bg-[#e0e0e0] mx-1" />

    {/* Actions */}
    <TBtn title={t('archive')} onClick={() => moveTo('archive')}><Archive size={18} /></TBtn>
    {currentFolder === 'spam'
      ? <TBtn title={t('not_spam', { defaultValue: 'Pas un spam' })} onClick={() => onSpam('inbox')}>
          <ShieldCheck size={18} />
        </TBtn>
      : <TBtn title={t('spam_report')} onClick={() => onSpam('spam')}>
          <ShieldAlert size={18} />
        </TBtn>}
    <TBtn title={t('delete')} danger onClick={onDelete}>
      <Trash2 size={18} />
    </TBtn>
    <div className="w-px h-5 bg-[#e0e0e0] mx-1" />

    {/* Mark as unread — first-class action, like Gmail */}
    <TBtn
      title={t('mail_mark_unread', { defaultValue: 'Marquer comme non lu' })}
      onClick={onThreadUnread}>
      <MailIcon size={18} />
    </TBtn>

    {/* Move to a folder */}
    <div className="relative">
      <TBtn title={t('move_to')} onClick={() => setMoveOpen(v => !v)}><FolderInput size={18} /></TBtn>
      {moveOpen && (
        <>
          <div className="fixed inset-0 z-40" onClick={() => setMoveOpen(false)} />
          <div className="absolute left-0 top-full mt-1 z-50 bg-white border border-border rounded-lg shadow-lg py-1 w-52">
            {([['inbox', t('folder_inbox')], ['archive', t('archive')], ['spam', t('folder_spam')], ['trash', t('folder_trash')]] as [string, string][])
              .filter(([f]) => f !== currentFolder)
              .map(([f, label]) => (
                <button key={f} onClick={() => { setMoveOpen(false); moveTo(f) }}
                  className="w-full flex items-center gap-2 px-3 py-2 text-sm text-text-primary hover:bg-surface-1 text-left">
                  <FolderInput size={14} className="text-text-tertiary" /> {label}
                </button>
              ))}
          </div>
        </>
      )}
    </div>

    {/* More options — Important / Mute / Snooze, like Gmail */}
    <button
      title={t('more_options', { defaultValue: "Plus d'options" })}
      onClick={e => { const r = e.currentTarget.getBoundingClientRect()
        setMorePos(p => p ? null : { top: r.bottom + 4, left: Math.max(8, r.left - 120), minWidth: 260 }) }}
      className="w-10 h-10 flex items-center justify-center rounded-full hover:bg-black/10 text-[#5f6368] transition-colors">
      <MoreVertical size={18} />
    </button>
    {morePos && <MenuDropdown pos={morePos} onClose={() => setMorePos(null)} items={moreItems} />}

    <div className="flex-1" />

    {/* Navigation */}
    <span className="text-xs text-[#444746] mr-1 select-none">
      {t('mail_message_count', { count: messageCount })}
    </span>
    <TBtn title={t('mail_older')} onClick={() => goRelative(1)}><ChevronLeft size={18} /></TBtn>
    <TBtn title={t('mail_newer')} onClick={() => goRelative(-1)}><ChevronRight size={18} /></TBtn>

    <div className="w-px h-5 bg-[#e0e0e0] mx-1" />
    <TBtn title={t('mail_open_new_window')} onClick={() => window.open(`${window.location.origin}/mail?thread=${thread.id}`, '_blank', 'noopener')}>
      <ExternalLink size={18} />
    </TBtn>
  </div>
  )
}
