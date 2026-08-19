import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useLocation } from 'react-router-dom'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Loader2, Mail as MailIcon, RefreshCw } from 'lucide-react'
import { useMenuDropdown, useIsMobile, type MenuDropdownPos } from '@ui'
import { ModuleServiceRegistry } from '@kubuno/sdk'
import { mailApi, Thread, type ThreadAttachment } from '../api'
import { usePullToRefresh } from './usePullToRefresh'
import { useMailStore } from '../store'
import { categoryFromHash } from '../categoryRoute'
import ThreadContextMenu from '../ThreadContextMenu'
import NewLabelDialog, { type NewLabelResult } from '../NewLabelDialog'
import { TABS, threadTab } from './categories'
import { plainKey } from './helpers'
import { createThreadActions, snoozePresets } from './threadActions'
import { patchThreadCaches, restoreThreadCaches, toggleStar, markRead } from './threadCache'
import ThreadItem from './ThreadItem'
import ThreadListToolbar from './ThreadListToolbar'
import CategoryTabs from './CategoryTabs'
import AttachmentPreview, { type PreviewSource } from './AttachmentPreview'
import MailFolderFilterBar, { type FilterFolder } from './MailFolderFilterBar'

// Folders that carry the Gmail-style filter chip bar (drafts has its own view).
const FILTER_FOLDERS = new Set<string>(['starred', 'important', 'sent', 'all', 'spam'])

// ── Thread list ───────────────────────────────────────────────────────────────

export default function ThreadList() {
  const { t } = useTranslation('mail')
  const { currentFolder, currentLabelId, currentImapFolder, inboxCategory, setInboxCategory, selectedAccount, selectedThread, setSelectedThread, searchQuery, accounts } = useMailStore()
  const isMobile = useIsMobile()
  const qc = useQueryClient()
  const [syncing,          setSyncing]          = useState(false)
  // Pull-to-refresh (touch only): drag the list past its top to reload it.
  const listRef = useRef<HTMLDivElement>(null)
  const reload = useCallback(async () => {
    await Promise.all([
      qc.refetchQueries({ queryKey: ['mail-threads'] }),
      qc.refetchQueries({ queryKey: ['mail-counts'] }),
    ])
  }, [qc])
  const pull = usePullToRefresh(listRef, reload, isMobile)
  const [checkedIds,       setCheckedIds]       = useState<Set<string>>(new Set())
  const [allChecked,       setAllChecked]       = useState(false)
  const [highlightedId,    setHighlightedId]    = useState<string | null>(null)

  const isStarred   = currentFolder === 'starred'
  const isLabel     = currentFolder === 'label'
  const isImportant = currentFolder === 'important'
  const isSnoozed   = currentFolder === 'snoozed'
  const special     = isStarred || isLabel || isImportant || isSnoozed
  const folder      = isStarred ? 'inbox' : currentFolder
  const isInbox     = currentFolder === 'inbox'
  // Categories (tabs) only filter the inbox.
  const activeTab = inboxCategory

  // The active category comes from the URL hash (/mail/#category/promotions), so
  // the link is shareable and back/forward work. No hash ⇒ 'main'.
  const { hash } = useLocation()
  useEffect(() => { setInboxCategory(categoryFromHash(hash)) }, [hash, setInboxCategory])

  // ── Gmail-style cursor pagination ("1–50 of N", ‹ › ) ───────────────────────
  const PAGE = 50
  const [pageIdx, setPageIdx] = useState(0)
  const [cursors, setCursors] = useState<(string | undefined)[]>([undefined]) // `before` per page
  // Back to the first page whenever the context changes.
  useEffect(() => { setPageIdx(0); setCursors([undefined]); setLoadedRows([]) },
    [currentFolder, currentLabelId, currentImapFolder, searchQuery, inboxCategory, selectedAccount])
  // The selection belongs to ONE view: switching folder/label/category/search
  // clears it (like Gmail). Opening a message and coming back does NOT change any
  // of these, so the selection — and the scroll position — survive that round trip
  // now that the list stays mounted under the reader.
  useEffect(() => { setCheckedIds(new Set()); setAllChecked(false) },
    [currentFolder, currentLabelId, currentImapFolder, searchQuery, inboxCategory, selectedAccount])
  const before = cursors[pageIdx]
  // MOBILE: pages are appended as the reader scrolls instead of being flipped
  // through, so the rows accumulate here. Desktop keeps one page at a time.
  const [loadedRows, setLoadedRows] = useState<Thread[]>([])

  // The inbox tab filter is applied SERVER-side, so each tab paginates its own
  // stream and gets its own total — like Gmail, where "1–50 of N" is per tab.
  const category = isInbox && !searchQuery ? activeTab : undefined

  const { data, isLoading, isFetching } = useQuery({
    queryKey: ['mail-threads', folder, currentLabelId, currentImapFolder, selectedAccount, isStarred, isImportant, isSnoozed, searchQuery, category, before],
    queryFn:  () => mailApi.listThreads({
      folder:     special ? undefined : folder,
      // Custom folders share the 'custom' bucket: the provider name narrows it.
      imap_folder: currentFolder === 'custom' ? (currentImapFolder ?? undefined) : undefined,
      category,
      label_id:   isLabel ? (currentLabelId ?? undefined) : undefined,
      account_id: selectedAccount ?? undefined,
      starred:    isStarred || undefined,
      important:  isImportant || undefined,
      snoozed:    isSnoozed || undefined,
      search:     searchQuery || undefined,
      before:     before ?? undefined,
      limit:      PAGE,
    }),
    // Switching tab/label changes the key: without this, `data` goes undefined
    // for one render, the range collapses to 0 and the total falls back to a
    // DIFFERENT source (the whole folder count) — a wrong number flashes before
    // the right one lands. Holding the previous result makes the swap atomic.
    placeholderData: previous => previous,
    refetchInterval: 60_000,
  })

  const hasMore = data?.has_more ?? false
  const goNext = () => {
    if (!hasMore || !data?.cursor) return
    setCursors(c => { const n = [...c]; n[pageIdx + 1] = data.cursor!; return n })
    setPageIdx(i => i + 1)
  }
  const goPrev = () => { if (pageIdx > 0) setPageIdx(i => i - 1) }

  const allThreads = data?.threads ?? []
  // Single source: the server counts exactly what it just listed, for every
  // view. The sidebar counters must NOT be used as a fallback — they count
  // unread threads, so a label with 3 read threads used to show "1–3 of 0".
  const total = data?.total ?? null
  const rangeStart = allThreads.length ? pageIdx * PAGE + 1 : 0
  const rangeEnd   = pageIdx * PAGE + allThreads.length

  // Category filtering happens in SQL now; rows arrive already scoped to the
  // active tab, so a second client-side pass would only risk hiding rows if
  // the two heuristics ever diverged.
  const threads = isMobile ? loadedRows : allThreads

  // Accumulate the pages behind the infinite scroll. Page 0 REPLACES (a refresh
  // must not duplicate the rows it just reloaded); later pages append, and ids
  // already present are skipped so a shifting cursor cannot double a row.
  useEffect(() => {
    if (!isMobile || !data?.threads) return
    setLoadedRows(prev => {
      if (pageIdx === 0) return data.threads
      const seen = new Set(prev.map(t => t.id))
      return [...prev, ...data.threads.filter(t => !seen.has(t.id))]
    })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [data, isMobile])

  // Reaching the bottom loads the next page — the mobile replacement for
  // "1–50 ‹ ›". A margin of one screen keeps the list from ever showing a gap.
  useEffect(() => {
    const el = listRef.current
    if (!el || !isMobile) return
    const onScroll = () => {
      if (!hasMore || isFetching) return
      if (el.scrollTop + el.clientHeight >= el.scrollHeight - el.clientHeight * 0.5) goNext()
    }
    el.addEventListener('scroll', onScroll, { passive: true })
    return () => el.removeEventListener('scroll', onScroll)
  })

  // Per-category tab data (unread count + preview). The list rows are now
  // scoped to one tab, so this reads the sidebar's cross-category query
  // (same key → shared cache with MailSidebarBody).
  const { data: inboxOverview } = useQuery({
    queryKey: ['mail-threads', 'inbox', null, null, false, ''],
    queryFn:  () => mailApi.listThreads({ folder: 'inbox', limit: 200 }),
    enabled:  isInbox && !searchQuery,
    refetchInterval: 60_000,
  })
  const catInfo = useMemo(() => {
    const info: Record<string, { count: number; preview: string }> = {}
    for (const tab of TABS) info[tab.id] = { count: 0, preview: '' }
    for (const th of inboxOverview?.threads ?? []) {
      if (th.unread_count <= 0) continue
      const cat = threadTab(th)
      const slot = info[cat]
      if (!slot) continue
      slot.count += 1
      if (!slot.preview) {
        const sender = th.last_sender_name || th.last_sender_email || '?'
        slot.preview = `${sender} — ${th.subject || t('mail_no_subject')}`
      }
    }
    return info
  }, [inboxOverview, t])

  // Preload every visible thread in memory
  useEffect(() => {
    for (const t of threads) {
      qc.prefetchQuery({
        queryKey: ['mail-thread', t.id],
        queryFn:  () => mailApi.getThread(t.id),
        staleTime: 2 * 60_000,
      })
    }
  }, [threads, qc])

  async function handleSync() {
    if (syncing) return
    setSyncing(true)
    try {
      const ids = selectedAccount ? [selectedAccount] : accounts.map(a => a.id)
      await Promise.all(ids.map(id => mailApi.triggerSync(id)))
      qc.invalidateQueries({ queryKey: ['mail-threads'] })
      await new Promise(r => setTimeout(r, 5000))
      qc.invalidateQueries({ queryKey: ['mail-threads'] })
    } finally { setSyncing(false) }
  }

  function toggleAll() {
    if (allChecked) { setCheckedIds(new Set()); setAllChecked(false) }
    else { setCheckedIds(new Set(threads.map(t => t.id))); setAllChecked(true) }
  }
  function toggleOne(id: string) {
    setCheckedIds(prev => {
      const next = new Set(prev)
      next.has(id) ? next.delete(id) : next.add(id)
      setAllChecked(next.size === threads.length && threads.length > 0)
      return next
    })
  }
  function selectBy(pred: (t: Thread) => boolean) {
    const next = new Set(threads.filter(pred).map(t => t.id))
    setCheckedIds(next); setAllChecked(next.size === threads.length && threads.length > 0); setSelMenuOpen(false)
  }

  const [selMenuOpen,    setSelMenuOpen]    = useState(false)
  // Mobile selection bar "⋮" menu (secondary bulk actions).
  const [listMorePos,    setListMorePos]    = useState<MenuDropdownPos | null>(null)
  const [bulkSnoozeOpen, setBulkSnoozeOpen] = useState(false)
  const [bulkMovePos,    setBulkMovePos]    = useState<MenuDropdownPos | null>(null)
  const [bulkMorePos,    setBulkMorePos]    = useState<MenuDropdownPos | null>(null)
  const [ctxCreateLabel, setCtxCreateLabel] = useState(false)
  const [preview, setPreview] = useState<PreviewSource | null>(null)

  /**
   * Clicking an attachment chip. Drive owns the real viewers, so hand the URL
   * over when it is installed; fall back to our own PDF/image surfaces when it
   * is not — a module may never assume a neighbour is there.
   *
   * The WHOLE set of the conversation's attachments goes with the request, plus
   * the index of the clicked one: Drive's previewer then walks them with its
   * « n / N ‹ › » control. An older Drive that only knows the single-source
   * shape returns false and we fall back, so nothing breaks either way.
   */
  const openAttachment = (att: ThreadAttachment, url: string, all: ThreadAttachment[]) => {
    const source = { url, name: att.name, mime: att.mime }
    const driveCan = ModuleServiceRegistry.call<boolean>('drive', 'canPreview', att.mime, att.name)
    if (driveCan) {
      const items = all.map(a => ({
        url:  mailApi.attachmentUrl(a.message_id, a.index),
        name: a.name,
        mime: a.mime,
      }))
      const index = all.findIndex(a => a.message_id === att.message_id && a.index === att.index)
      const request = items.length > 1 && index >= 0 ? { items, index } : source
      if (ModuleServiceRegistry.call<boolean>('drive', 'openPreview', request)) return
      // Older Drive: retry with the single-source shape it understands.
      if (request !== source && ModuleServiceRegistry.call<boolean>('drive', 'openPreview', source)) return
    }

    if (att.mime === 'application/pdf' || att.mime.startsWith('image/')) {
      setPreview(source)
    } else {
      window.open(url, '_blank', 'noopener')
    }
  }
  // `composeOpen` / the two manager modals are read only to keep the keyboard
  // shortcuts from acting on the list hidden behind them.
  const { setPendingCompose, setSearchQuery, composeOpen, templatesOpen, groupsOpen } = useMailStore()
  const { data: labelsData } = useQuery({ queryKey: ['mail-labels'], queryFn: mailApi.listLabels })
  // Right-click menu on a row.
  const ctxMenu = useMenuDropdown()
  const [ctxThread, setCtxThread] = useState<Thread | null>(null)

  const refreshLists = () => {
    qc.invalidateQueries({ queryKey: ['mail-threads'] })
    qc.invalidateQueries({ queryKey: ['mail-counts'] })
  }
  const clearSel = () => { setCheckedIds(new Set()); setAllChecked(false) }
  const selIds = () => [...checkedIds]
  const presets = () => snoozePresets(t)

  const startCompose = (thread: Thread, mode: 'reply' | 'replyAll' | 'forward') => {
    setPendingCompose({ threadId: thread.id, mode })
    setSelectedThread(thread.id)
  }

  const {
    doArchive, doDelete, doRead, doImportant, doSnooze, doSpam, doMute, doMoveToLabel, ctxActions,
  } = createThreadActions({
    t,
    clearSel,
    qc,
    refreshLists,
    closeBulkSnooze: () => setBulkSnoozeOpen(false),
    closeBulkMove:   () => setBulkMovePos(null),
    startCompose,
    onCreateLabel:   () => setCtxCreateLabel(true),
    setSearchQuery,
  })

  /** A mixed selection reads as unread, so one click marks everything read. */
  const selectionHasUnread = threads.some(th => checkedIds.has(th.id) && th.unread_count > 0)
  const labelList = labelsData?.labels?.filter(l => !l.is_system) ?? []

  // List shortcuts (active while no conversation is open): j/k to navigate,
  // Enter/o to open, e to archive, #/Backspace to delete, s to star, x to select.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!plainKey(e) || selectedThread) return
      const idx = threads.findIndex(tt => tt.id === highlightedId)
      const hid = highlightedId
      if (e.key === 'j' || e.key === 'ArrowDown') {
        const n = threads[Math.min((idx < 0 ? -1 : idx) + 1, threads.length - 1)]; if (n) { setHighlightedId(n.id); e.preventDefault() }
      } else if (e.key === 'k' || e.key === 'ArrowUp') {
        const n = threads[Math.max((idx < 0 ? 1 : idx) - 1, 0)]; if (n) { setHighlightedId(n.id); e.preventDefault() }
      } else if ((e.key === 'Enter' || e.key === 'o') && hid) { e.preventDefault(); setSelectedThread(hid) }
      else if (e.key === 'x' && hid) { e.preventDefault(); toggleOne(hid) }
      else if (e.key === 's' && hid) {
        e.preventDefault()
        const snapshot = patchThreadCaches(qc, [hid], toggleStar)
        mailApi.starThread(hid)
          .catch(() => restoreThreadCaches(qc, snapshot))
          .finally(refreshLists)
      }
      else if (e.key === 'e' && hid) { e.preventDefault(); doArchive([hid]) }
      else if ((e.key === '#' || e.key === 'Backspace') && hid) { e.preventDefault(); doDelete([hid]) }
      // Delete: exactly what the row menu's "Supprimer" does — the ticked
      // conversations when the selection is not empty, the highlighted row
      // otherwise. Never while a composer, a manager modal, the row menu or a
      // preview is up: they own the keyboard.
      else if (e.key === 'Delete') {
        if (composeOpen || templatesOpen || groupsOpen || ctxMenu.pos || ctxCreateLabel || preview) return
        const ids = checkedIds.size > 0 ? [...checkedIds] : hid ? [hid] : []
        if (ids.length === 0) return
        e.preventDefault()
        doDelete(ids)
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [threads, highlightedId, selectedThread, checkedIds, composeOpen, templatesOpen, groupsOpen,
      ctxMenu.pos, ctxCreateLabel, preview]) // eslint-disable-line react-hooks/exhaustive-deps

  return (
    <div className="flex flex-col bg-white overflow-hidden flex-1 min-w-0">

      <ThreadListToolbar
        isMobile={isMobile}
        threads={threads}
        labelList={labelList}
        syncing={syncing}
        isFetching={isFetching}
        onSync={handleSync}
        checkedIds={checkedIds}
        allChecked={allChecked}
        toggleAll={toggleAll}
        clearSel={clearSel}
        selIds={selIds}
        selectBy={selectBy}
        selectionHasUnread={selectionHasUnread}
        selMenuOpen={selMenuOpen}
        setSelMenuOpen={setSelMenuOpen}
        listMorePos={listMorePos}
        setListMorePos={setListMorePos}
        bulkMovePos={bulkMovePos}
        setBulkMovePos={setBulkMovePos}
        bulkMorePos={bulkMorePos}
        setBulkMorePos={setBulkMorePos}
        doArchive={doArchive}
        doDelete={doDelete}
        doRead={doRead}
        doImportant={doImportant}
        doSnooze={doSnooze}
        doSpam={doSpam}
        doMute={doMute}
        doMoveToLabel={doMoveToLabel}
        snoozePresets={presets}
        rangeStart={rangeStart}
        rangeEnd={rangeEnd}
        total={total}
        pageIdx={pageIdx}
        hasMore={hasMore}
        goPrev={goPrev}
        goNext={goNext}
      />

      {FILTER_FOLDERS.has(currentFolder) && (
        <MailFolderFilterBar folder={currentFolder as FilterFolder} />
      )}

      <CategoryTabs
        visible={isInbox && !searchQuery}
        isMobile={isMobile}
        activeTab={activeTab}
        catInfo={catInfo}
        onCategorized={refreshLists}
      />

      {/* ── List ─────────────────────────────────────────────────────────── */}
      {/* `overscroll-contain` stops the browser's own overscroll refresh from
          competing with the gesture handled below. */}
      <div ref={listRef} className="flex-1 overflow-y-auto overscroll-contain relative">
        {/* Pull-to-refresh indicator: follows the finger, spins while loading. */}
        {(pull.offset > 0 || pull.refreshing) && (
          <div
            className="absolute left-0 right-0 flex justify-center pointer-events-none z-10"
            style={{ top: 4, transform: `translateY(${pull.offset - 28}px)`,
                     transition: pull.offset === 0 ? 'transform .2s ease-out' : undefined }}
          >
            <div className="w-9 h-9 rounded-full bg-white shadow-md flex items-center justify-center">
              <RefreshCw
                size={17}
                className={pull.refreshing ? 'animate-spin text-primary' : pull.armed ? 'text-primary' : 'text-text-tertiary'}
                style={pull.refreshing ? undefined : { transform: `rotate(${pull.offset * 4}deg)` }}
              />
            </div>
          </div>
        )}
        {isLoading ? (
          <div className="flex justify-center py-12">
            <Loader2 size={22} className="animate-spin text-text-tertiary" />
          </div>
        ) : threads.length === 0 ? (
          <div className="flex flex-col items-center justify-center py-16 text-text-tertiary">
            <MailIcon size={36} className="mb-3 opacity-30" />
            <p className="text-sm">{t('mail_no_messages')}</p>
          </div>
        ) : (
          threads.map(thread => (
            <ThreadItem
              key={thread.id}
              thread={thread}
              highlighted={highlightedId === thread.id}
              opened={selectedThread === thread.id}
              checked={checkedIds.has(thread.id)}
              selectionActive={checkedIds.size > 0}
              onSingleClick={() => {
                setHighlightedId(thread.id)
                setSelectedThread(thread.id)
                // Opening marks the conversation read server-side (get_thread),
                // so drop the bold here instead of waiting for the refetch.
                if (thread.unread_count > 0) patchThreadCaches(qc, [thread.id], markRead(true))
              }}
              onCheck={e => { e.stopPropagation(); toggleOne(thread.id) }}
              onArchive={() => doArchive([thread.id])}
              onDelete={() => doDelete([thread.id])}
              onMarkRead={() => doRead([thread.id], thread.unread_count > 0)}
              onSnooze={() => doSnooze([thread.id], presets()[1].until)}
              onContextMenu={e => { e.preventDefault(); setCtxThread(thread); ctxMenu.open(e) }}
              onDragThreads={() => (checkedIds.has(thread.id) ? [...checkedIds] : [thread.id])}
              onPreviewAttachment={openAttachment}
            />
          ))
        )}

        {/* Infinite scroll: the next page is loading below the last row. */}
        {isMobile && isFetching && pageIdx > 0 && (
          <div className="flex justify-center py-4">
            <Loader2 size={18} className="animate-spin text-text-tertiary" />
          </div>
        )}
        {/* Nothing left to load — say so rather than leaving the reader pulling. */}
        {isMobile && !hasMore && threads.length > 0 && (
          <div className="py-4 text-center text-xs text-text-tertiary">
            {t('mail_end_of_list', { defaultValue: 'Fin de la liste' })}
          </div>
        )}
      </div>

      {preview && (
        <AttachmentPreview preview={preview} onClose={() => setPreview(null)} />
      )}

      {ctxMenu.pos && ctxThread && (
        <ThreadContextMenu
          thread={ctxThread}
          labels={labelList}
          pos={ctxMenu.pos}
          onClose={ctxMenu.close}
          actions={ctxActions(ctxThread)}
        />
      )}

      {ctxCreateLabel && (
        <NewLabelDialog
          labels={labelList}
          onCancel={() => setCtxCreateLabel(false)}
          onCreate={async ({ name }: NewLabelResult) => {
            const accountId = ctxThread?.account_id ?? accounts[0]?.id
            if (accountId) await mailApi.createLabel({ account_id: accountId, name }).catch(() => {})
            setCtxCreateLabel(false)
            qc.invalidateQueries({ queryKey: ['mail-labels'] })
          }}
        />
      )}
    </div>
  )
}
