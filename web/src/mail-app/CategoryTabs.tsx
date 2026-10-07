import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link as RouterLink } from 'react-router-dom'
import { mailApi } from '../api'
import { categoryTo } from '../categoryRoute'
import { isThreadDrag, readDraggedThreads, setThreadDropCaption, THREAD_DND_TYPE } from '../threadDnd'
import { TABS } from './categories'

export type CategoryInfo = Record<string, { count: number; preview: string }>

// ── Category tabs — inbox only ────────────────────────────────────────────────

export default function CategoryTabs({
  visible, isMobile, activeTab, catInfo, onCategorized,
}: {
  /** Rendered but hidden outside the (unsearched) inbox, like before. */
  visible:       boolean
  isMobile:      boolean
  activeTab:     string
  catInfo:       CategoryInfo
  /** Refresh the lists once conversations have been dropped on a tab. */
  onCategorized: () => void
}) {
  const { t } = useTranslation('mail')
  const [dropTab, setDropTab] = useState<string | null>(null)
  // True while a conversation is being dragged anywhere in the window, so the
  // tabs can advertise themselves as drop targets like Gmail does.
  const [dragging, setDragging] = useState(false)
  useEffect(() => {
    const on  = (e: DragEvent) => { if (e.dataTransfer?.types.includes(THREAD_DND_TYPE)) setDragging(true) }
    const off = () => setDragging(false)
    // Bubble phase on purpose: in capture, this runs BEFORE the row's own
    // handler has put the payload on the dataTransfer, so the test would fail.
    document.addEventListener('dragstart', on, false)
    document.addEventListener('dragend', off, true)
    document.addEventListener('drop', off, true)
    return () => {
      document.removeEventListener('dragstart', on, false)
      document.removeEventListener('dragend', off, true)
      document.removeEventListener('drop', off, true)
    }
  }, [])

  return (
    <div className={`${visible ? 'flex' : 'hidden'} border-b border-[#e0e0e0] overflow-x-auto scrollbar-none flex-shrink-0 bg-white`}>
      {TABS.map(tab => {
        const isActive = activeTab === tab.id
        const { count, preview } = catInfo[tab.id]
        // Active tab shows just icon + title (Gmail hides the preview there);
        // inactive tabs with unread show the count badge + last-message preview.
        const showDetail = !isActive && count > 0
        return (
          <RouterLink
            key={tab.id}
            to={categoryTo(tab.id)}
            onDragOver={e => {
              if (!isThreadDrag(e)) return
              e.preventDefault()
              e.dataTransfer.dropEffect = 'move'
              setDropTab(tab.id)
              setThreadDropCaption(t(tab.labelKey))
            }}
            onDragLeave={() => { setDropTab(null); setThreadDropCaption(null) }}
            onDrop={async e => {
              const ids = readDraggedThreads(e)
              setDropTab(null)
              setThreadDropCaption(null)
              if (!ids.length) return
              e.preventDefault()
              for (const id of ids) await mailApi.setThreadCategory(id, tab.id).catch(() => {})
              onCategorized()
            }}
            title={showDetail ? preview : undefined}
            /* Indicator, hover and widths mirror the core `Tabs` primitive:
             * - the band is a ::before, not a bottom border, so it gets rounded top
             *   corners and an 8px inset from the tab's edges;
             * - hover tints EVERY tab, the active one included;
             * - desktop tabs are 256px wide and shrink no further than 128px, while
             *   their content stays left-aligned.
             * Mobile keeps its own sizing: 256px tabs on a phone would leave one and
             * a half visible. */
            className={`relative flex items-center text-left transition-colors
              before:absolute before:inset-x-0 before:bottom-0 before:mx-2
              before:h-[3px] before:rounded-t-[3px] before:content-['']
              ${isMobile ? 'flex-shrink-0 gap-2 px-4 h-12 min-w-0' : 'gap-3 pl-4 pr-6 h-14 w-[256px] min-w-[128px]'}
              hover:bg-surface-2
              ${dropTab === tab.id ? 'bg-[#fef7e0]' : isActive ? 'before:bg-primary' : ''}`}
          >
            <tab.Icon size={isMobile ? 17 : 20} className={`flex-shrink-0 ${isActive ? 'text-primary' : 'text-text-tertiary'}`} />
            <span className="flex flex-col min-w-0 leading-tight">
              <span className="flex items-center gap-2 min-w-0">
                <span className={`text-[15px] truncate ${isActive ? 'text-primary font-medium' : 'text-text-primary'}`}>
                  {t(tab.labelKey)}
                </span>
                {showDetail && (
                  <span className={`text-[11px] font-semibold px-1.5 py-0.5 rounded leading-none whitespace-nowrap flex-shrink-0 ${tab.badge}`}>
                    {t('mail_new_count', { count })}
                  </span>
                )}
              </span>
              {dragging && !isActive && !isMobile ? (
                <span className="text-xs text-text-tertiary truncate">
                  {t('mail_drag_here', { defaultValue: 'Faire glisser ici' })}
                </span>
              ) : showDetail && !isMobile ? (
                <span className="text-xs text-text-tertiary truncate">{preview}</span>
              ) : null}
            </span>
          </RouterLink>
        )
      })}
    </div>
  )
}
