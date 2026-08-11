import type { TFunction } from 'i18next'
import type { QueryClient } from '@tanstack/react-query'
import { ModuleServiceRegistry } from '@kubuno/sdk'
import { mailApi, Thread, type Label } from '../api'
import type { ThreadMenuActions } from '../ThreadContextMenu'
import {
  patchThreadCaches, restoreThreadCaches, markRead, toggleImportant, removeRow,
} from './threadCache'

export type SnoozePreset = { label: string; until: string }

export function snoozePresets(t: TFunction): SnoozePreset[] {
  const now = new Date()
  const later = new Date(now); later.setHours(now.getHours() + 3, 0, 0, 0)
  const tom   = new Date(now); tom.setDate(now.getDate() + 1); tom.setHours(8, 0, 0, 0)
  const week  = new Date(now); week.setDate(now.getDate() + 7); week.setHours(8, 0, 0, 0)
  return [
    { label: t('snooze_later',    { defaultValue: 'Plus tard (3 h)' }),      until: later.toISOString() },
    { label: t('snooze_tomorrow', { defaultValue: 'Demain' }),               until: tom.toISOString() },
    { label: t('snooze_nextweek', { defaultValue: 'La semaine prochaine' }), until: week.toISOString() },
  ]
}

/**
 * Every mutation the thread LIST can run: the bulk actions of the toolbar, the
 * per-row hover actions, and the right-click menu of a row. They all clear the
 * selection and refresh the lists, so they are built together from the state
 * setters the list owns.
 */
export function createThreadActions({
  t, qc, clearSel, refreshLists, closeBulkSnooze, closeBulkMove,
  startCompose, onCreateLabel, setSearchQuery,
}: {
  t:               TFunction
  qc:              QueryClient
  clearSel:        () => void
  refreshLists:    () => void
  closeBulkSnooze: () => void
  closeBulkMove:   () => void
  startCompose:    (thread: Thread, mode: 'reply' | 'replyAll' | 'forward') => void
  onCreateLabel:   () => void
  setSearchQuery:  (q: string) => void
}) {
  /**
   * Runs a batch with the lists already updated. The rows react on the next
   * frame; the requests follow, and the lists are revalidated once they are
   * all done. A failure puts the previous rows back.
   */
  const optimistic = async (
    ids: string[],
    patch: (thread: Thread) => Thread | null,
    request: (id: string) => Promise<unknown>,
    after?: () => void,
  ) => {
    const snapshot = patchThreadCaches(qc, ids, patch)
    clearSel()
    after?.()
    try {
      await Promise.all(ids.map(id => request(id)))
    } catch (e) {
      restoreThreadCaches(qc, snapshot)
      throw e
    } finally {
      refreshLists()
    }
  }

  // Bulk + single-row actions (reused by the row hover actions).
  const doArchive   = (ids: string[]) => optimistic(ids, removeRow, id => mailApi.moveThread(id, 'archive'))
  const doDelete    = (ids: string[]) => optimistic(ids, removeRow, id => mailApi.deleteThread(id))
  const doRead      = (ids: string[], r: boolean) => optimistic(ids, markRead(r), id => mailApi.readThread(id, r))
  const doImportant = (ids: string[]) => optimistic(ids, toggleImportant, id => mailApi.importantThread(id))
  const doSnooze    = (ids: string[], until: string) => optimistic(ids, removeRow, id => mailApi.snoozeThread(id, until), closeBulkSnooze)
  const doSpam      = (ids: string[]) => optimistic(ids, removeRow, id => mailApi.moveThread(id, 'spam'), closeBulkMove)
  const doMute      = (ids: string[]) => optimistic(ids, removeRow, id => mailApi.muteThread(id))
  // "Move to a label" is Gmail's label + archive, same as the row menu.
  const doMoveToLabel = async (ids: string[], label: Label) => {
    for (const id of ids) {
      await mailApi.addLabel(id, label.id).catch(() => {})
      await mailApi.moveThread(id, 'archive').catch(() => {})
    }
    clearSel(); closeBulkMove(); refreshLists()
  }

  // ── Context menu actions ──────────────────────────────────────────────────
  const ctxActions = (thread: Thread): ThreadMenuActions => ({
    onReply:      () => startCompose(thread, 'reply'),
    onReplyAll:   () => startCompose(thread, 'replyAll'),
    onForward:    () => startCompose(thread, 'forward'),
    onArchive:    () => doArchive([thread.id]),
    onDelete:     () => doDelete([thread.id]),
    onToggleRead: () => doRead([thread.id], thread.unread_count > 0),
    onSnooze:     () => doSnooze([thread.id], snoozePresets(t)[1].until),
    onAddToTasks: async () => {
      await ModuleServiceRegistry.call<Promise<unknown>>('tasks', 'createTask', { title: thread.subject })
    },
    onMoveFolder: async folder => { await mailApi.moveThread(thread.id, folder); refreshLists() },
    // "Move to a label" is Gmail's archive + label in one go.
    onMoveLabel: async label => {
      await mailApi.addLabel(thread.id, label.id).catch(() => {})
      await mailApi.moveThread(thread.id, 'archive').catch(() => {})
      refreshLists()
    },
    onToggleLabel: async label => {
      const has = (thread.labels ?? []).some(l => l.id === label.id)
      await (has ? mailApi.removeLabel(thread.id, label.id) : mailApi.addLabel(thread.id, label.id)).catch(() => {})
      refreshLists()
    },
    onCreateLabel: onCreateLabel,
    onMute:        async () => { await mailApi.muteThread(thread.id).catch(() => {}); refreshLists() },
    onSearchFrom:  () => setSearchQuery(`from:${thread.last_sender_email}`),
    onOpenWindow:  () => window.open(`/mail?thread=${thread.id}`, '_blank', 'noopener'),
  })

  return { doArchive, doDelete, doRead, doImportant, doSnooze, doSpam, doMute, doMoveToLabel, ctxActions }
}
