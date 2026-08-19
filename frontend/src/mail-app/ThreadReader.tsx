import { Fragment, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Loader2, Printer, ExternalLink, ShieldAlert, ChevronsUpDown } from 'lucide-react'
import { useIsMobile } from '@ui'
import { mailApi, EmailMessage, type Thread } from '../api'
import { useMailStore } from '../store'
import { plainKey } from './helpers'
import { ImportanceMarker } from './rowChrome'
import { snoozePresets } from './threadActions'
import {
  patchThreadCaches, restoreThreadCaches, markRead, toggleImportant, removeRow,
  type ThreadCacheSnapshot,
} from './threadCache'
import MessageCard from './MessageCard'
import InlineCompose from './InlineCompose'
import ThreadReaderToolbar from './ThreadReaderToolbar'

// ── Thread reader ─────────────────────────────────────────────────────────────

export default function ThreadReader({ onOpenPdf }: { onOpenPdf: (url: string, name: string) => void }) {
  const { t } = useTranslation('mail')
  // `composeOpen`: a floating composer is a surface of its own — the reader's
  // shortcuts must not act on the conversation behind it.
  const { selectedThread, setSelectedThread, currentFolder, composeOpen } = useMailStore()
  const qc = useQueryClient()
  const [inlineMode, setInlineMode] = useState<'reply' | 'forward' | null>(null)
  // Which messages are open. Held HERE rather than in each card so the
  // "expand/collapse all" button reflects what the reader opened by hand.
  const [expandedIds, setExpandedIds] = useState<Set<string>>(new Set())
  const [inlineMsg,  setInlineMsg]  = useState<EmailMessage | null>(null)
  const [replyAll,   setReplyAll]   = useState(false)

  useEffect(() => { setInlineMode(null); setInlineMsg(null) }, [selectedThread])

  const { data, isLoading } = useQuery({
    queryKey: ['mail-thread', selectedThread],
    queryFn:  () => mailApi.getThread(selectedThread!),
    enabled:  !!selectedThread,
  })

  // Opening a thread (or loading its messages): only the last one starts open.
  useEffect(() => {
    const msgs = data?.messages ?? []
    const last = msgs[msgs.length - 1]
    setExpandedIds(new Set(last ? [last.id] : []))
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedThread, data?.messages?.length])

  /*
   * Opening a conversation marks it read — ONCE per opening, and only from
   * here. The GET used to do it, but it is prefetched for every visible row,
   * so entire pages were marked read on their own and a message put back to
   * unread flipped straight back at the next prefetch.
   *
   * The ref is what keeps "mark this message unread" (which leaves the reader
   * open) from being undone a render later; it is cleared when the reader
   * closes, so reopening the thread marks it read again.
   */
  const markedRead = useRef<string | null>(null)
  useEffect(() => {
    if (!selectedThread) { markedRead.current = null; return }
    if (markedRead.current === selectedThread) return
    const thread = data?.thread
    if (!thread || thread.id !== selectedThread) return
    markedRead.current = selectedThread
    if ((thread.unread_count ?? 0) <= 0) return

    patchThreadCaches(qc, [selectedThread], markRead(true))
    mailApi.readThread(selectedThread, true)
      .then(() => qc.invalidateQueries({ queryKey: ['mail-counts'] }))
      .catch(() => qc.invalidateQueries({ queryKey: ['mail-threads'] }))
  }, [selectedThread, data, qc])

  // Reply/forward asked for from the list's right-click menu: the thread had
  // to load first, so the request waits in the store until its messages arrive.
  const pendingCompose = useMailStore(s => s.pendingCompose)
  const setPendingCompose = useMailStore(s => s.setPendingCompose)
  useEffect(() => {
    if (!pendingCompose || pendingCompose.threadId !== selectedThread) return
    const msgs = data?.messages ?? []
    const last = msgs[msgs.length - 1]
    if (!last) return
    setInlineMsg(last)
    setInlineMode(pendingCompose.mode === 'forward' ? 'forward' : 'reply')
    setReplyAll(pendingCompose.mode === 'replyAll')
    setPendingCompose(null)
  }, [pendingCompose, selectedThread, data, setPendingCompose])

  /**
   * Reader actions apply to the UI first — the reader closes and the list row
   * updates on the next frame — then the request runs. Waiting for the response
   * before closing made every one of them feel like a server round trip.
   */
  const optimistic = <V,>(
    request: (v: V) => Promise<unknown>,
    patch: (thread: Thread) => Thread | null,
    opts: { close?: boolean } = { close: true },
  ) => ({
    mutationFn: request,
    onMutate: (v: V) => {
      const id = typeof v === 'string' ? v : (v as { id: string }).id
      const snapshot = patchThreadCaches(qc, [id], patch)
      const openThread = selectedThread
      if (opts.close !== false) setSelectedThread(null)
      return { snapshot, openThread }
    },
    onError: (_e: unknown, _v: V, ctx?: { snapshot: ThreadCacheSnapshot; openThread: string | null }) => {
      if (!ctx) return
      restoreThreadCaches(qc, ctx.snapshot)
      if (opts.close !== false) setSelectedThread(ctx.openThread)
    },
    onSettled: () => {
      qc.invalidateQueries({ queryKey: ['mail-threads'] })
      qc.invalidateQueries({ queryKey: ['mail-counts'] })
    },
  })

  // Marking the whole conversation unread closes it, like Gmail: it just went
  // back to the unread pile, so keeping it open would flip it straight to read.
  const threadUnreadMut = useMutation(
    optimistic<string>(id => mailApi.readThread(id, false), markRead(false)),
  )

  const deleteMut = useMutation(optimistic<string>(id => mailApi.deleteThread(id), removeRow))

  const deleteMessageMut = useMutation({
    mutationFn: (id: string) => mailApi.deleteMessage(id),
    onSuccess:  () => qc.invalidateQueries({ queryKey: ['mail-thread', selectedThread] }),
  })

  // Per-message: the thread itself goes back to unread, but the reader stays open.
  const markUnreadMut = useMutation({
    mutationFn: (id: string) => mailApi.markRead(id, false),
    onMutate:   () => (selectedThread ? patchThreadCaches(qc, [selectedThread], markRead(false)) : undefined),
    onError:    (_e, _v, snapshot) => { if (snapshot) restoreThreadCaches(qc, snapshot) },
    onSettled:  () => {
      qc.invalidateQueries({ queryKey: ['mail-thread', selectedThread] })
      qc.invalidateQueries({ queryKey: ['mail-threads'] })
    },
  })

  // Importance also has to touch the READER's own cache (`['mail-thread', id]`):
  // `optimistic` only patches the list caches, so the marker and the details
  // card kept the old value until a refetch happened.
  const importantMut = useMutation({
    mutationFn: (id: string) => mailApi.importantThread(id),
    onMutate: (id: string) => {
      const snapshot = patchThreadCaches(qc, [id], toggleImportant)
      qc.setQueryData(['mail-thread', id], (old: { thread: Thread } | undefined) =>
        old ? { ...old, thread: { ...old.thread, is_important: !old.thread.is_important } } : old)
      return snapshot
    },
    onError: (_e, id, snapshot?: ThreadCacheSnapshot) => {
      if (snapshot) restoreThreadCaches(qc, snapshot)
      qc.invalidateQueries({ queryKey: ['mail-thread', id] })
    },
    onSettled: (_d, _e, id) => {
      qc.invalidateQueries({ queryKey: ['mail-thread', id] })
      qc.invalidateQueries({ queryKey: ['mail-threads'] })
      qc.invalidateQueries({ queryKey: ['mail-counts'] })
    },
  })

  const snoozeMut = useMutation(
    optimistic<{ id: string; until: string | null }>(
      ({ id, until }) => mailApi.snoozeThread(id, until),
      removeRow,
    ),
  )
  const [snoozeOpen, setSnoozeOpen] = useState(false)

  // Mark as spam (folder='spam') or "not spam" (folder='inbox'): moves the
  // thread AND trains the Bayesian classifier on the backend side.
  const spamMut = useMutation(
    optimistic<{ id: string; folder: 'spam' | 'inbox' }>(
      ({ id, folder }) => mailApi.moveThread(id, folder),
      removeRow,
    ),
  )

  const muteMut = useMutation(optimistic<string>(id => mailApi.muteThread(id), removeRow))

  const presets = () => snoozePresets(t)

  // Move the thread to a folder (archive, trash…) then go back to the list.
  const moveTo = (folder: string) => {
    mailApi.moveThread(selectedThread!, folder).then(() => {
      setSelectedThread(null)
      qc.invalidateQueries({ queryKey: ['mail-threads'] })
      qc.invalidateQueries({ queryKey: ['mail-counts'] })
    })
  }

  // Previous / next thread: located in the already loaded list (React Query cache).
  const goRelative = (dir: 1 | -1) => {
    const pages = qc.getQueriesData<{ threads: { id: string }[] }>({ queryKey: ['mail-threads'] })
    const list = pages.flatMap(([, d]) => d?.threads ?? [])
    const idx = list.findIndex(th => th.id === selectedThread)
    const next = idx >= 0 ? list[idx + dir] : undefined
    if (next) setSelectedThread(next.id)
  }

  // Reader shortcuts: Escape/u=back, e=archive, #/Backspace/Delete=delete,
  // r=reply, f=forward.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!plainKey(e) || !selectedThread || !data) return
      const { thread, messages } = data
      switch (e.key) {
        case 'Escape': case 'u': e.preventDefault(); setSelectedThread(null); break
        case 'e': e.preventDefault(); mailApi.moveThread(thread.id, 'archive').then(() => {
          setSelectedThread(null); qc.invalidateQueries({ queryKey: ['mail-threads'] }); qc.invalidateQueries({ queryKey: ['mail-counts'] })
        }); break
        // Same deletion as the toolbar's trash button, on the open conversation.
        case 'Delete': if (composeOpen) break; e.preventDefault(); deleteMut.mutate(thread.id); break
        case '#': case 'Backspace': e.preventDefault(); deleteMut.mutate(thread.id); break
        case 'r': { e.preventDefault(); const m = messages[messages.length - 1]; if (m) { setInlineMode('reply'); setInlineMsg(m) } break }
        case 'f': { e.preventDefault(); const m = messages[messages.length - 1]; if (m) { setInlineMode('forward'); setInlineMsg(m) } break }
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [selectedThread, data, deleteMut, qc, setSelectedThread, composeOpen])

  const isMobile = useIsMobile()

  if (isLoading) {
    return (
      <div className="flex-1 flex items-center justify-center bg-white">
        <Loader2 size={24} className="animate-spin text-text-tertiary" />
      </div>
    )
  }

  const { thread, messages } = data!

  // The button's label follows the REAL state, including messages the reader
  // opened one by one. The last message is always open (not collapsible).
  const lastId = messages[messages.length - 1]?.id
  const allExpanded = messages.length > 0 && messages.every(m => expandedIds.has(m.id) || m.id === lastId)
  const toggleAll = () => setExpandedIds(
    allExpanded ? new Set(lastId ? [lastId] : []) : new Set(messages.map(m => m.id)),
  )

  return (
    <div className="flex-1 flex flex-col min-w-0 overflow-hidden bg-white">

      <ThreadReaderToolbar
        isMobile={isMobile}
        thread={thread}
        messageCount={messages.length}
        currentFolder={currentFolder}
        onBack={() => setSelectedThread(null)}
        moveTo={moveTo}
        onDelete={() => deleteMut.mutate(thread.id)}
        onSpam={folder => spamMut.mutate({ id: thread.id, folder })}
        onThreadUnread={() => threadUnreadMut.mutate(thread.id)}
        onImportant={() => importantMut.mutate(thread.id)}
        onMute={() => muteMut.mutate(thread.id)}
        onSnooze={until => { setSnoozeOpen(false); snoozeMut.mutate({ id: thread.id, until }) }}
        snoozePresets={presets}
        goRelative={goRelative}
      />

      {/* ══ Reading area (scrollable) ═════════════════════════════════════════ */}
      <div className="flex-1 overflow-y-auto">
        <div className={`w-full py-4 ${isMobile ? 'px-4' : 'px-8'}`}>

          {/* ── Subject + label ───────────────────────────────────────────── */}
          {/* Indented by the avatar column (40px + 12px gap) so the subject lines
              up with the sender names and the message bodies below it. */}
          <div className={`flex items-start gap-3 mb-4 ${isMobile ? '' : 'ps-[52px]'}`}>
            {/* Subject, importance marker and folder chip read as one line, the
                way Gmail groups them; the tools stay on the far right. */}
            <div className="flex-1 min-w-0 flex items-center gap-2 flex-wrap">
              <h1 className={`font-normal text-[#202124] leading-snug break-words ${isMobile ? 'text-lg' : 'text-[22px]'}`}>
                {thread.subject || t('mail_no_subject')}
              </h1>
              <button
                onClick={() => importantMut.mutate(thread.id)}
                className="w-7 h-7 flex-shrink-0 flex items-center justify-center rounded-full hover:bg-black/[0.08] transition-colors"
                title={thread.is_important
                  ? t('mail_mark_not_important', { defaultValue: 'Marquer comme non important' })
                  : t('mail_mark_important',     { defaultValue: 'Marquer comme important' })}
              >
                <ImportanceMarker active={!!thread.is_important} />
              </button>
              <span className="flex items-center gap-1 text-[11px] text-[#444746] bg-[#f1f3f4]
                               px-2.5 py-1 rounded border border-[#dadce0] whitespace-nowrap">
                {t('folder_inbox')}
                {/* × = remove from the inbox (archive), Gmail style. */}
                <button className="ml-1 hover:text-[#202124] leading-none" title={t('archive')}
                  onClick={() => moveTo('archive')}>×</button>
              </span>
            </div>
            <div className="flex items-center gap-2 flex-shrink-0 mt-1">
              {/* Expand / collapse every message — only useful past one message. */}
              {messages.length > 1 && (
                <button
                  className="p-1.5 rounded-full hover:bg-[#f1f3f4] text-[#444746]"
                  title={allExpanded
                    ? t('mail_collapse_all', { defaultValue: 'Tout réduire' })
                    : t('mail_expand_all',   { defaultValue: 'Tout développer' })}
                  onClick={toggleAll}
                >
                  <ChevronsUpDown size={16} />
                </button>
              )}
              {/* Print / new window: secondary — on mobile they live in "⋮" only. */}
              {!isMobile && <>
                <button className="p-1.5 rounded-full hover:bg-[#f1f3f4] text-[#444746]" title={t('print')}
                  onClick={() => window.print()}>
                  <Printer size={16} />
                </button>
                <button className="p-1.5 rounded-full hover:bg-[#f1f3f4] text-[#444746]" title={t('mail_new_window')}
                  onClick={() => window.open(`${window.location.origin}/mail?thread=${thread.id}`, '_blank', 'noopener')}>
                  <ExternalLink size={16} />
                </button>
              </>}
            </div>
          </div>

          {/* Bayesian warning: message kept in the inbox but judged suspicious. */}
          {currentFolder === 'inbox' && messages.some(m => (m.spam_score ?? 0) >= 0.7) && (
            <div className="mb-3 flex items-center gap-2 px-3 py-2 rounded-lg bg-amber-50 border border-amber-200 text-xs text-amber-800">
              <ShieldAlert size={16} className="flex-shrink-0 text-amber-500" />
              <span className="flex-1">{t('spam_suspected', { defaultValue: 'Ce message ressemble à un indésirable.' })}</span>
              <button
                onClick={() => spamMut.mutate({ id: thread.id, folder: 'spam' })}
                className="px-2.5 py-1 rounded-md bg-amber-500 text-white text-xs font-medium hover:bg-amber-600"
              >
                {t('spam_report')}
              </button>
            </div>
          )}

          {/* ── Messages ──────────────────────────────────────────────────── */}
          {messages.map((msg, i) => (
            <Fragment key={msg.id}>
              {/* Full-bleed hairline BETWEEN messages: negative margins cancel the
                  container padding so the line reaches both edges, as Gmail's does. */}
              {i > 0 && <div className={`border-t border-[#e8eaed] ${isMobile ? '-mx-4' : '-mx-8'}`} />}
            <MessageCard
              message={msg}
              important={!!thread.is_important}
              isLast={i === messages.length - 1}
              // The LAST message stays open: it is the one being read, and a lone
              // message (which is also the last) has nothing to collapse into.
              collapsible={i !== messages.length - 1}
              expanded={expandedIds.has(msg.id) || msg.id === lastId}
              onToggle={open => setExpandedIds(prev => {
                const next = new Set(prev)
                if (open) next.add(msg.id); else next.delete(msg.id)
                return next
              })}
              onReply={() => { setReplyAll(false); setInlineMode('reply'); setInlineMsg(msg) }}
              onReplyAll={() => { setReplyAll(true); setInlineMode('reply'); setInlineMsg(msg) }}
              onForward={() => { setInlineMode('forward'); setInlineMsg(msg) }}
              onDelete={() => deleteMessageMut.mutate(msg.id)}
              onMarkUnread={() => markUnreadMut.mutate(msg.id)}
              onOpenPdf={onOpenPdf}
              onSpam={() => spamMut.mutate({ id: thread.id, folder: 'spam' })}
            />
            </Fragment>
          ))}

          {/* ── Inline reply area ───────────────────────────────────────── */}
          {inlineMode && inlineMsg && (
            <div className="mt-4 mb-2">
              <InlineCompose
                mode={inlineMode}
                message={inlineMsg}
                replyAll={replyAll}
                onSent={() => { setInlineMode(null); setReplyAll(false) }}
                onCancel={() => { setInlineMode(null); setReplyAll(false) }}
              />
            </div>
          )}

          <div className="h-8" />
        </div>
      </div>
    </div>
  )
}
