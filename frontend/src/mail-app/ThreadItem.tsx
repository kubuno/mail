import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import {
  Paperclip, Check, Archive, Trash2, MailOpen, Mail as MailIcon, Clock,
} from 'lucide-react'
import { useIsMobile } from '@ui'
import { mailApi, Thread, type ThreadAttachment } from '../api'
import { useMailStore } from '../store'
import { useSwipeActions } from '../useSwipe'
import { unsubscribeTarget } from '../MailViews'
import { startThreadDrag } from '../threadDnd'
import ThreadChips from '../ThreadChips'
import ThreadAttachmentChips from '../ThreadAttachmentChips'
import { formatDate } from './helpers'
import SenderAvatar from './SenderAvatar'
import { analyzeSender, sanitizeDisplayText } from './senderSafety'
import { patchThreadCaches, restoreThreadCaches, toggleStar, toggleImportant } from './threadCache'
import {
  SelectBox, DragGrip, RowAction, ListStar, ImportanceMarker, markOpacity,
  ROW_CHECKED_BG, ROW_HIGHLIGHT_BG, ROW_READ_BG, ROW_HOVER_SHADOW, ROW_SEPARATOR_SHADOW,
} from './rowChrome'

export default function ThreadItem({
  thread, highlighted, opened, checked, selectionActive = false, onSingleClick, onCheck,
  onArchive, onDelete, onMarkRead, onSnooze, onContextMenu, onDragThreads, onPreviewAttachment,
}: {
  thread:         Thread
  highlighted:    boolean
  opened:         boolean
  checked:        boolean
  /** At least one conversation is selected: on touch, tapping a row then adds
   *  it to the selection instead of opening it (until nothing is left). */
  selectionActive?: boolean
  onSingleClick:  () => void
  onCheck:        (e: React.MouseEvent) => void
  onArchive:      () => void
  onDelete:       () => void
  onMarkRead:     () => void
  onSnooze:       () => void
  onContextMenu:  (e: React.MouseEvent) => void
  /** Ids to drag: the whole selection when this row belongs to it. */
  onDragThreads:  () => string[]
  /** Previews one attachment, with the row's whole set for ←/→ navigation. */
  onPreviewAttachment: (att: ThreadAttachment, url: string, all: ThreadAttachment[]) => void
}) {
  const { t, i18n } = useTranslation('mail')
  const [hovered, setHovered] = useState(false)
  const qc = useQueryClient()
  const density = useMailStore(s => s.density)
  const currentFolder  = useMailStore(s => s.currentFolder)
  const currentLabelId = useMailStore(s => s.currentLabelId)
  const unread = thread.unread_count > 0
  // A list row has no room for a warning chip, so the safe label IS the warning:
  // bidi/control characters are stripped, and a display name impersonating
  // another address is replaced by the address the mail really came from.
  const sender = analyzeSender(thread.last_sender_name, thread.last_sender_email)
  const senderDisplay = sender.label || '?'

  // Optimistic: the star/marker flips on the next frame. Waiting for the
  // response and then for a full list refetch made these feel like they were
  // talking to the mail server — they are one UPDATE away.
  const starMut = useMutation({
    mutationFn: () => mailApi.starThread(thread.id),
    onMutate:   () => patchThreadCaches(qc, [thread.id], toggleStar),
    onError:    (_e, _v, snapshot) => { if (snapshot) restoreThreadCaches(qc, snapshot) },
    onSettled:  () => qc.invalidateQueries({ queryKey: ['mail-threads'] }),
  })

  const importantMut = useMutation({
    mutationFn: () => mailApi.importantThread(thread.id),
    onMutate:   () => patchThreadCaches(qc, [thread.id], toggleImportant),
    onError:    (_e, _v, snapshot) => { if (snapshot) restoreThreadCaches(qc, snapshot) },
    onSettled:  () => qc.invalidateQueries({ queryKey: ['mail-threads'] }),
  })

  // A single click opens the conversation — no double-click debounce. Touch and
  // mouse behave the same way.
  //
  // ONE exception, on touch: while a selection is under way, tapping a row adds
  // it to (or removes it from) that selection rather than opening it. There is
  // no modifier key to hold on a phone, so the selection itself is the mode —
  // and it lasts until the last conversation is unselected.
  function handleClick(e?: React.MouseEvent | React.KeyboardEvent) {
    if (isMobile && selectionActive) {
      onCheck(e as React.MouseEvent)
      return
    }
    onSingleClick()
  }

  const hasChips = (thread.attachments ?? []).length > 0

  // Senders exposing List-Unsubscribe get Gmail's inline shortcut on hover.
  const unsubUrl = thread.list_unsubscribe ? unsubscribeTarget(thread.list_unsubscribe) : null

  const swipe = useSwipeActions({ onRight: onArchive, onLeft: onDelete })
  const isMobile = useIsMobile()

  // Mobile: Gmail-style two-line row — avatar (tap = select), sender + date,
  // subject/snippet + star. Hover actions are replaced by the swipe gestures.
  if (isMobile) {
    return (
      <div
        role="button"
        tabIndex={0}
        onClick={handleClick}
        onKeyDown={e => { if (e.key === 'Enter') handleClick(e) }}
        {...swipe.handlers}
        style={swipe.dx !== 0 ? { transform: `translateX(${swipe.dx}px)`, transition: swipe.swiping ? 'none' : 'transform 0.2s ease', touchAction: 'pan-y' } : { touchAction: 'pan-y' }}
        className={`relative flex items-center gap-3 px-4 py-2 min-h-[64px] cursor-pointer select-none border-b border-[#f0f0f0]
          ${swipe.dx > 0 ? 'bg-[#1e8e3e]' : swipe.dx < 0 ? 'bg-[#d93025]' :
            opened ? 'bg-blue-50' : checked ? 'bg-[#e8f0fe]' : 'bg-white'}`}
      >
        {/* Avatar — tap toggles selection */}
        {/* Avatar — kept on MOBILE (it is also the selection control here), while
            the desktop list stays text-only. A tap toggles selection. */}
        <button
          onClick={e => { e.stopPropagation(); onCheck(e) }}
          className="w-10 h-10 rounded-full flex items-center justify-center text-white flex-shrink-0"
          style={checked ? { backgroundColor: '#1a73e8' } : undefined}
        >
          {checked
            ? <Check size={18} />
            : <SenderAvatar email={thread.last_sender_email} name={thread.last_sender_name} size={40} />}
        </button>
        <div className="flex-1 min-w-0">
          <div className="flex items-baseline gap-2">
            <span title={sender.full} className={`flex-1 truncate text-[15px] ${unread ? 'font-semibold text-text-primary' : 'text-text-secondary'}`}>
              {senderDisplay}
            </span>
            <span className={`text-xs flex-shrink-0 ${unread ? 'font-semibold text-primary' : 'text-text-tertiary'}`}>
              {formatDate(thread.last_message_at, t, i18n.language)}
            </span>
          </div>
          <div className="flex items-center gap-2 min-w-0">
            <div className="flex-1 min-w-0">
              <div className={`truncate text-sm ${unread ? 'font-semibold text-text-primary' : 'text-text-primary'}`}>
                {sanitizeDisplayText(thread.subject) || t('mail_no_subject')}
              </div>
              {thread.snippet && (
                <div className="truncate text-xs text-text-tertiary">{sanitizeDisplayText(thread.snippet)}</div>
              )}
            </div>
            {thread.has_attachments && <Paperclip size={14} className="text-text-tertiary flex-shrink-0" />}
            {/* Touch target and mark are both bigger here than on desktop: a
                thumb needs room, and an 18px star read as an afterthought. */}
            <button onClick={e => { e.stopPropagation(); starMut.mutate() }} className="p-1.5 -m-1 flex-shrink-0"
              title={thread.is_starred ? t('mail_unstar') : t('mail_star')}>
              <ListStar active={thread.is_starred} size={24} />
            </button>
          </div>
        </div>
      </div>
    )
  }

  return (
    <div
      role="button"
      tabIndex={0}
      onClick={handleClick}
      onContextMenu={onContextMenu}
      onKeyDown={e => { if (e.key === 'Enter') handleClick() }}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      {...swipe.handlers}
      // Uniform row spacing comes from real vertical padding around the 20px
      // content line: 10px (6px compact) top and bottom, whether or not the row
      // grows with attachment chips. The oversized hover pills (28px star and
      // importance, 40px row actions) carry negative margins so they never
      // inflate the row beyond text + padding (40px, Gmail parity).
      // `items-start`, not `items-center`: when attachment chips add a second
      // line to the subject column, sender, date and hover actions must stay on
      // the FIRST line with the subject instead of re-centring against the
      // taller row. Every item on that line is a 20px line box (or a taller
      // hover pill with matching negative margins), so single-line rows are
      // laid out exactly as before.
      className={`relative flex items-start ${density === 'compact' ? 'py-[6px]' : 'py-[10px]'}
        pl-4 pr-4 gap-0 cursor-pointer select-none group ${hovered ? 'z-[2]' : ''}`}
      // Backgrounds and separator follow Gmail exactly: READ rows carry the
      // blue-grey tint and UNREAD ones stay white (not the other way round), the
      // row line is an inset shadow, and hovering only lifts the row — the
      // background never changes under the pointer.
      style={{
        ...(swipe.dx !== 0
          ? { transform: `translateX(${swipe.dx}px)`, transition: swipe.swiping ? 'none' : 'transform 0.2s ease' }
          : null),
        touchAction:     'pan-y',
        backgroundColor: swipe.dx > 0 ? '#1e8e3e' : swipe.dx < 0 ? '#d93025'
          : checked     ? ROW_CHECKED_BG
          : highlighted || opened ? ROW_HIGHLIGHT_BG
          : unread      ? '#ffffff'
          : ROW_READ_BG,
        boxShadow: hovered ? ROW_HOVER_SHADOW : ROW_SEPARATOR_SHADOW,
      }}
    >
      {/* Drag grip — appears on hover in the 13px slot left of the checkbox,
          and really drags the conversation onto a sidebar label. */}
      <span
        draggable
        onDragStart={e => {
          const ids = onDragThreads()
          startThreadDrag(e, ids, t('mail_move_n_threads', {
            count: ids.length,
            defaultValue: `Déplacer ${ids.length} conversation${ids.length > 1 ? 's' : ''}`,
          }))
        }}
        onClick={e => e.stopPropagation()}
        title={t('mail_drag_thread', { defaultValue: 'Faire glisser la conversation' })}
        className={`absolute left-[3px] w-[10px] h-5 flex items-center justify-center cursor-grab
                    active:cursor-grabbing ${hovered ? 'opacity-100' : 'opacity-0'}`}
      >
        <DragGrip />
      </span>

      {/* Gutter — glyph centres at 26/56/86 like Gmail, each in its own
          circular hover pill (28px) that never shifts the layout. */}
      <div className="w-5 h-5 flex-shrink-0 flex items-center justify-center">
        <SelectBox
          checked={checked}
          onClick={onCheck}
          label={t('mail_select_thread', { defaultValue: 'Sélectionner' })}
          opacity={markOpacity(checked, hovered)}
        />
      </div>

      <button
        onClick={e => { e.stopPropagation(); starMut.mutate() }}
        className="w-7 h-7 my-[-4px] ml-[6px] flex-shrink-0 flex items-center justify-center rounded-full
                   hover:bg-black/[0.08] transition-colors"
        title={thread.is_starred ? t('mail_unstar') : t('mail_star')}
      >
        <ListStar active={thread.is_starred} opacity={markOpacity(thread.is_starred, hovered)} />
      </button>

      <button
        onClick={e => { e.stopPropagation(); importantMut.mutate() }}
        className="w-7 h-7 my-[-4px] ml-[2px] flex-shrink-0 flex items-center justify-center rounded-full
                   hover:bg-black/[0.08] transition-colors"
        title={thread.is_important
          ? t('mail_mark_not_important', { defaultValue: 'Marquer comme non important' })
          : t('mail_mark_important', { defaultValue: 'Marquer comme important' })}
      >
        <ImportanceMarker active={!!thread.is_important} opacity={markOpacity(!!thread.is_important, hovered)} />
      </button>

      {/* Sender — 200px column with 32px of breathing room, text starting at 106 */}
      <div title={sender.full} className={`flex-shrink-0 truncate text-sm w-[200px] ml-[6px] pr-8
        ${unread ? 'font-bold text-text-primary' : 'font-normal text-text-primary'}`}>
        {senderDisplay}
      </div>

      {/* Subject + snippet, preceded by the folder/label chips */}
      <div className={`flex-1 min-w-0 ${hasChips ? 'flex flex-col gap-1' : ''}`}>
      <div className="flex items-center gap-1.5 overflow-hidden min-h-5">
        <ThreadChips thread={thread} currentFolder={currentFolder} currentLabelId={currentLabelId} />
        <span className={`text-sm truncate flex-shrink-0 max-w-[60%]
          ${unread ? 'font-bold text-text-primary' : 'text-text-primary'}`}>
          {sanitizeDisplayText(thread.subject) || t('mail_no_subject')}
        </span>
        {thread.snippet && (
          <span className="text-sm text-[#5f6368] truncate ml-1">
            &nbsp;–&nbsp;{sanitizeDisplayText(thread.snippet)}
          </span>
        )}
        {thread.has_attachments && !hasChips && (
          <Paperclip size={13} className="text-text-tertiary ml-2 flex-shrink-0" />
        )}
      </div>
      {hasChips && (
        <ThreadAttachmentChips
          attachments={thread.attachments!}
          onPreview={(att, url) => onPreviewAttachment(att, url, thread.attachments!)}
        />
      )}
      </div>

      {/* Hover actions (Gmail style: Archive / Delete / Read-Unread / Snooze) */}
      {hovered && (
        <div className="flex items-center h-5 flex-shrink-0 ml-2" onClick={e => e.stopPropagation()}>
          {unsubUrl && (
            <a
              href={unsubUrl}
              target="_blank"
              rel="noopener noreferrer"
              onClick={e => e.stopPropagation()}
              className="mr-2 h-5 px-2 inline-flex items-center rounded-[4px] whitespace-nowrap
                         text-xs font-medium text-[#444746] no-underline hover:bg-black/[0.08]"
            >
              {t('mail_unsubscribe')}
            </a>
          )}
          <RowAction onClick={onArchive} title={t('archive', { defaultValue: 'Archiver' })}><Archive size={16} /></RowAction>
          <RowAction onClick={onDelete} title={t('delete')}><Trash2 size={16} /></RowAction>
          <RowAction
            onClick={onMarkRead}
            title={t(thread.unread_count > 0 ? 'mail_mark_read' : 'mail_mark_unread', { defaultValue: 'Marquer comme lu' })}>
            {thread.unread_count > 0 ? <MailOpen size={16} /> : <MailIcon size={16} />}
          </RowAction>
          <RowAction onClick={onSnooze} title={t('mail_snooze_action')}><Clock size={16} /></RowAction>
        </div>
      )}

      {/* Date */}
      <div className={`text-xs leading-5 flex-shrink-0 text-right min-w-[64px] ml-2
        ${unread ? 'font-bold text-text-primary' : 'text-[#5f6368]'}
        ${hovered ? 'hidden' : ''}`}>
        {formatDate(thread.last_message_at, t, i18n.language)}
      </div>
    </div>
  )
}
