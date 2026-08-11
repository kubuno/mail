import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'
import {
  RefreshCw, ChevronLeft, ChevronRight, ChevronDown, X, MoreVertical,
  Mail as MailIcon, MailOpen, Trash2, Archive, ShieldAlert, Clock, Bookmark,
  FolderInput, BellOff, AlignJustify, Settings2, Columns2, Rows2, Square,
} from 'lucide-react'
import { MenuDropdown, type MenuDropdownPos } from '@ui'
import { Thread, type Label } from '../api'
import { useMailStore } from '../store'
import { SelectBox } from './rowChrome'
import type { SnoozePreset } from './threadActions'

export default function ThreadListToolbar({
  isMobile, threads, labelList, syncing, isFetching, onSync,
  checkedIds, allChecked, toggleAll, clearSel, selIds, selectBy, selectionHasUnread,
  selMenuOpen, setSelMenuOpen, listMorePos, setListMorePos,
  bulkMovePos, setBulkMovePos, bulkMorePos, setBulkMorePos,
  doArchive, doDelete, doRead, doImportant, doSnooze, doSpam, doMute, doMoveToLabel,
  snoozePresets, rangeStart, rangeEnd, total, pageIdx, hasMore, goPrev, goNext,
}: {
  isMobile:   boolean
  threads:    Thread[]
  labelList:  Label[]
  syncing:    boolean
  isFetching: boolean
  onSync:     () => void
  checkedIds: Set<string>
  allChecked: boolean
  toggleAll:  () => void
  clearSel:   () => void
  selIds:     () => string[]
  selectBy:   (pred: (t: Thread) => boolean) => void
  /** A mixed selection reads as unread, so one click marks everything read. */
  selectionHasUnread: boolean
  selMenuOpen:    boolean
  setSelMenuOpen: React.Dispatch<React.SetStateAction<boolean>>
  listMorePos:    MenuDropdownPos | null
  setListMorePos: React.Dispatch<React.SetStateAction<MenuDropdownPos | null>>
  bulkMovePos:    MenuDropdownPos | null
  setBulkMovePos: React.Dispatch<React.SetStateAction<MenuDropdownPos | null>>
  bulkMorePos:    MenuDropdownPos | null
  setBulkMorePos: React.Dispatch<React.SetStateAction<MenuDropdownPos | null>>
  doArchive:     (ids: string[]) => void
  doDelete:      (ids: string[]) => void
  doRead:        (ids: string[], r: boolean) => void
  doImportant:   (ids: string[]) => void
  doSnooze:      (ids: string[], until: string) => void
  doSpam:        (ids: string[]) => void
  doMute:        (ids: string[]) => void
  doMoveToLabel: (ids: string[], label: Label) => void
  snoozePresets: () => SnoozePreset[]
  rangeStart: number
  rangeEnd:   number
  total:      number | null
  pageIdx:    number
  hasMore:    boolean
  goPrev:     () => void
  goNext:     () => void
}) {
  const { t } = useTranslation('mail')
  const navigate = useNavigate()
  const { splitMode, setSplitMode, density, setDensity } = useMailStore()
  const [splitMenuPos, setSplitMenuPos] = useState<MenuDropdownPos | null>(null)

  const TBtn = ({ onClick, title, children }: { onClick: () => void; title: string; children: React.ReactNode }) => (
    <button onClick={onClick} title={title}
      className="w-10 h-10 flex items-center justify-center rounded-full
                 hover:bg-black/10 text-[#5f6368] transition-colors">{children}</button>
  )

  // ── Toolbar ─────────────────────────────────────────────────────────────
  // Mobile: slim bar — refresh + pagination when browsing; X + count +
  // 3 primary bulk actions + "⋮" while selecting (avatar tap selects).
  // Desktop: full Gmail-style bar, unchanged.
  // MOBILE: no toolbar at all while nothing is selected — reloading is the
  // pull-to-refresh gesture and paging is the infinite scroll, so the bar had
  // only two buttons left that duplicated them. It comes back as the selection
  // action bar as soon as a conversation is ticked.
  if (isMobile && checkedIds.size === 0) return null

  return isMobile ? (
    <div className="flex items-center gap-1 px-2 h-11 border-b border-[#f0f0f0] flex-shrink-0 no-print">
      {checkedIds.size > 0 ? (
        <>
          <TBtn onClick={clearSel} title={t('sel_none', { defaultValue: 'Aucun' })}><X size={18} /></TBtn>
          <span className="text-xs font-medium text-text-primary">{checkedIds.size}</span>
          <div className="flex-1" />
          <TBtn onClick={() => doArchive(selIds())} title={t('archive', { defaultValue: 'Archiver' })}><Archive size={18} /></TBtn>
          <TBtn onClick={() => doDelete(selIds())} title={t('delete', { defaultValue: 'Supprimer' })}><Trash2 size={18} /></TBtn>
          <TBtn onClick={() => doRead(selIds(), true)} title={t('mail_mark_read', { defaultValue: 'Marquer comme lu' })}><MailOpen size={18} /></TBtn>
          <button
            title={t('more_options')}
            onClick={e => { const r = e.currentTarget.getBoundingClientRect(); setListMorePos(p => p ? null : { top: r.bottom + 4, left: Math.max(8, r.right - 240), minWidth: 240 }) }}
            className="p-1.5 rounded hover:bg-surface-2 text-text-tertiary transition-colors">
            <MoreVertical size={18} />
          </button>
          {listMorePos && (
            <MenuDropdown pos={listMorePos} onClose={() => setListMorePos(null)} items={[
              { type: 'action', icon: <MailIcon size={15} />, label: t('mail_mark_unread', { defaultValue: 'Marquer comme non lu' }), onClick: () => doRead(selIds(), false) },
              { type: 'action', icon: <Bookmark size={15} />, label: t('folder_important', { defaultValue: 'Important' }), onClick: () => doImportant(selIds()) },
              { type: 'submenu', icon: <Clock size={15} />, label: t('snooze', { defaultValue: 'Différer' }),
                items: snoozePresets().map(p => ({ type: 'action' as const, label: p.label, onClick: () => doSnooze(selIds(), p.until) })) },
              { type: 'separator' },
              { type: 'action', label: t('sel_all', { defaultValue: 'Tout sélectionner' }), onClick: () => selectBy(() => true) },
            ]} />
          )}
        </>
      ) : (
        <>
          <TBtn onClick={onSync} title={t('mail_refresh')}>
            <RefreshCw size={16} className={(syncing || isFetching) ? 'animate-spin' : ''} />
          </TBtn>
          <div className="flex-1" />
          <span className="text-xs text-text-secondary tabular-nums">{`${rangeStart}–${rangeEnd}`}</span>
          <button onClick={goPrev} disabled={pageIdx === 0}
            title={t('mail_newer', { defaultValue: 'Plus récents' })}
            className="p-1.5 rounded hover:bg-surface-2 text-text-secondary disabled:opacity-30 transition-colors">
            <ChevronLeft size={16} />
          </button>
          <button onClick={goNext} disabled={!hasMore}
            title={t('mail_older', { defaultValue: 'Plus anciens' })}
            className="p-1.5 rounded hover:bg-surface-2 text-text-secondary disabled:opacity-30 transition-colors">
            <ChevronRight size={16} />
          </button>
        </>
      )}
    </div>
  ) : (
  <div className="flex items-center flex-wrap gap-0 pl-4 pr-4 h-12 flex-shrink-0 no-print">
    {/* Checkbox + selection menu (All / None / Read / Unread / Starred) */}
    <div className="relative flex items-center w-10 flex-shrink-0">
      <SelectBox
        checked={allChecked}
        partial={checkedIds.size > 0 && !allChecked}
        onClick={toggleAll}
        label={t('mail_select_all')}
      />
      <button onClick={() => setSelMenuOpen(v => !v)} className="px-0.5 text-text-tertiary hover:text-text-primary">
        <ChevronDown size={14} />
      </button>
      {selMenuOpen && (
        <>
          <div className="fixed inset-0 z-40" onClick={() => setSelMenuOpen(false)} />
          <div className="absolute left-0 top-full mt-1 z-50 bg-white border border-border rounded-lg shadow-lg py-1 w-44 text-sm">
            <button className="w-full px-3 py-1.5 text-left hover:bg-surface-1" onClick={() => selectBy(() => true)}>{t('sel_all', { defaultValue: 'Tout' })}</button>
            <button className="w-full px-3 py-1.5 text-left hover:bg-surface-1" onClick={() => { clearSel(); setSelMenuOpen(false) }}>{t('sel_none', { defaultValue: 'Aucun' })}</button>
            <button className="w-full px-3 py-1.5 text-left hover:bg-surface-1" onClick={() => selectBy(tt => tt.unread_count === 0)}>{t('sel_read', { defaultValue: 'Lus' })}</button>
            <button className="w-full px-3 py-1.5 text-left hover:bg-surface-1" onClick={() => selectBy(tt => tt.unread_count > 0)}>{t('sel_unread', { defaultValue: 'Non lus' })}</button>
            <button className="w-full px-3 py-1.5 text-left hover:bg-surface-1" onClick={() => selectBy(tt => tt.is_starred)}>{t('sel_starred', { defaultValue: 'Suivis' })}</button>
          </div>
        </>
      )}
    </div>
    <button
      onClick={onSync}
      disabled={syncing}
      title={t('mail_refresh')}
      className="w-10 h-10 ml-[10px] flex items-center justify-center rounded-full
                 hover:bg-black/10 text-[#5f6368] disabled:opacity-50 transition-colors"
    >
      <RefreshCw size={20} className={(syncing || isFetching) ? 'animate-spin' : ''} />
    </button>
    {checkedIds.size === 0 && (
      <>
        <button
          title={t('more_options', { defaultValue: "Plus d'options" })}
          onClick={e => { const r = e.currentTarget.getBoundingClientRect()
            setListMorePos(p => p ? null : { top: r.bottom + 4, left: r.left, minWidth: 240 }) }}
          className="w-10 h-10 flex items-center justify-center rounded-full
                     hover:bg-black/10 text-[#5f6368] transition-colors">
          <MoreVertical size={20} />
        </button>
        {listMorePos && (
          <MenuDropdown pos={listMorePos} onClose={() => setListMorePos(null)} items={[
            { type: 'action', icon: <MailOpen size={15} />,
              label: t('mail_mark_all_read', { defaultValue: 'Tout marquer comme lu' }),
              onClick: () => doRead(threads.filter(x => x.unread_count > 0).map(x => x.id), true) },
            { type: 'separator' },
            { type: 'action', icon: <AlignJustify size={15} />,
              label: density === 'compact'
                ? t('density_comfortable', { defaultValue: 'Affichage normal' })
                : t('density_compact', { defaultValue: 'Affichage compact' }),
              onClick: () => setDensity(density === 'compact' ? 'comfortable' : 'compact') },
            { type: 'action', icon: <Settings2 size={15} />,
              label: t('mail_settings', { defaultValue: 'Paramètres' }),
              onClick: () => navigate('/mail/settings') },
          ]} />
        )}
      </>
    )}

    {/* Bulk actions — same set and order as Gmail:
        Archive · Spam · Delete | Mark read | Move to | More */}
    {checkedIds.size > 0 && (
      <>
        <div className="w-px h-5 bg-[#e0e0e0] mx-2" />
        <TBtn onClick={() => doArchive(selIds())} title={t('archive', { defaultValue: 'Archiver' })}><Archive size={20} /></TBtn>
        <TBtn onClick={() => doSpam(selIds())} title={t('mail_report_spam', { defaultValue: 'Signaler comme spam' })}><ShieldAlert size={20} /></TBtn>
        <TBtn onClick={() => doDelete(selIds())} title={t('delete', { defaultValue: 'Supprimer' })}><Trash2 size={20} /></TBtn>
        <div className="w-px h-5 bg-[#e0e0e0] mx-2" />
        <TBtn
          onClick={() => doRead(selIds(), selectionHasUnread)}
          title={selectionHasUnread
            ? t('mail_mark_read', { defaultValue: 'Marquer comme lu' })
            : t('mail_mark_unread', { defaultValue: 'Marquer comme non lu' })}>
          {selectionHasUnread ? <MailOpen size={20} /> : <MailIcon size={20} />}
        </TBtn>
        <button
          title={t('move_to', { defaultValue: 'Déplacer vers' })}
          onClick={e => { const r = e.currentTarget.getBoundingClientRect()
            setBulkMovePos(p => p ? null : { top: r.bottom + 4, left: r.left, minWidth: 260 }) }}
          className="w-10 h-10 flex items-center justify-center rounded-full hover:bg-black/10 text-[#5f6368] transition-colors">
          <FolderInput size={20} />
        </button>
        <button
          title={t('more_options', { defaultValue: "Plus d'options" })}
          onClick={e => { const r = e.currentTarget.getBoundingClientRect()
            setBulkMorePos(p => p ? null : { top: r.bottom + 4, left: Math.max(8, r.left - 120), minWidth: 240 }) }}
          className="w-10 h-10 flex items-center justify-center rounded-full hover:bg-black/10 text-[#5f6368] transition-colors">
          <MoreVertical size={20} />
        </button>

        {bulkMovePos && (
          <MenuDropdown pos={bulkMovePos} onClose={() => setBulkMovePos(null)} items={[
            ...labelList.map(l => ({
              type: 'action' as const, label: l.name,
              onClick: () => doMoveToLabel(selIds(), l),
            })),
            ...(labelList.length ? [{ type: 'separator' as const }] : []),
            { type: 'action', label: t('folder_spam',  { defaultValue: 'Spam' }),      onClick: () => doSpam(selIds()) },
            { type: 'action', label: t('folder_trash', { defaultValue: 'Corbeille' }), onClick: () => doDelete(selIds()) },
          ]} />
        )}

        {bulkMorePos && (
          <MenuDropdown pos={bulkMorePos} onClose={() => setBulkMorePos(null)} items={[
            { type: 'action', icon: <MailIcon size={15} />, label: t('mail_mark_unread', { defaultValue: 'Marquer comme non lu' }), onClick: () => doRead(selIds(), false) },
            { type: 'action', icon: <Bookmark size={15} />, label: t('folder_important', { defaultValue: 'Important' }), onClick: () => doImportant(selIds()) },
            { type: 'submenu', icon: <Clock size={15} />, label: t('mail_snooze_action'),
              items: snoozePresets().map(p => ({ type: 'action' as const, label: p.label, onClick: () => doSnooze(selIds(), p.until) })) },
            { type: 'action', icon: <BellOff size={15} />, label: t('mail_mute'), onClick: () => doMute(selIds()) },
            { type: 'separator' },
            { type: 'action', label: t('sel_all', { defaultValue: 'Tout sélectionner' }), onClick: () => selectBy(() => true) },
          ]} />
        )}
      </>
    )}

    <div className="flex-1" />
    {checkedIds.size > 0 && (
      <span className="text-xs text-text-secondary mr-2">{t('mail_selected_count', { count: checkedIds.size })}</span>
    )}
    {/* Gmail-style pagination: "1–50 of N" + ‹ › */}
    <div className="flex items-center gap-0.5 text-xs text-text-secondary flex-shrink-0">
      <span className="mr-1 tabular-nums hidden sm:inline">
        {total != null
          ? t('mail_range_of', { start: rangeStart, end: rangeEnd, total, defaultValue: `${rangeStart}–${rangeEnd} sur ${total}` })
          : `${rangeStart}–${rangeEnd}`}
      </span>
      <button onClick={goPrev} disabled={pageIdx === 0}
        title={t('mail_newer', { defaultValue: 'Plus récents' })}
        className="w-10 h-10 flex items-center justify-center rounded-full hover:bg-black/10
                   disabled:opacity-30 disabled:hover:bg-transparent transition-colors">
        <ChevronLeft size={20} />
      </button>
      <button onClick={goNext} disabled={!hasMore}
        title={t('mail_older', { defaultValue: 'Plus anciens' })}
        className="w-10 h-10 flex items-center justify-center rounded-full hover:bg-black/10
                   disabled:opacity-30 disabled:hover:bg-transparent transition-colors">
        <ChevronRight size={20} />
      </button>

      {/* Display density (comfortable / compact) — desktop only */}
      <button
        onClick={() => setDensity(density === 'compact' ? 'comfortable' : 'compact')}
        title={t('density_toggle', { defaultValue: density === 'compact' ? 'Affichage normal' : 'Affichage compact' })}
        className={`hidden lg:block p-1.5 rounded hover:bg-surface-2 transition-colors ${density === 'compact' ? 'text-primary' : ''}`}>
        <AlignJustify size={16} />
      </button>

      {/* Split-pane mode — hidden on mobile (no room for panes) */}
      <div className="ml-1 hidden lg:block">
        <button onClick={e => { const r = e.currentTarget.getBoundingClientRect(); setSplitMenuPos(p => p ? null : { top: r.bottom + 4, left: r.right - 224 }) }}
          title={t('split_toggle', { defaultValue: 'Mode Volet Double' })}
          className={`flex items-center p-1.5 rounded hover:bg-surface-2 transition-colors ${splitMode !== 'none' ? 'text-primary' : ''}`}>
          {splitMode === 'horizontal' ? <Rows2 size={16} /> : <Columns2 size={16} />}
          <ChevronDown size={12} className="-ml-0.5" />
        </button>
        {splitMenuPos && (
          <MenuDropdown
            pos={{ ...splitMenuPos, minWidth: 224 }}
            onClose={() => setSplitMenuPos(null)}
            items={[
              { type: 'action', icon: <Square size={15} />,   label: t('split_none', { defaultValue: 'Aucune séparation' }),       checked: splitMode === 'none',       onClick: () => setSplitMode('none') },
              { type: 'action', icon: <Columns2 size={15} />, label: t('split_vertical', { defaultValue: 'Séparation verticale' }),  checked: splitMode === 'vertical',   onClick: () => setSplitMode('vertical') },
              { type: 'action', icon: <Rows2 size={15} />,    label: t('split_horizontal', { defaultValue: 'Séparation horizontale' }), checked: splitMode === 'horizontal', onClick: () => setSplitMode('horizontal') },
            ]}
          />
        )}
      </div>
    </div>
  </div>
  )
}
