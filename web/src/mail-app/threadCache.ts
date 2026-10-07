import type { QueryClient } from '@tanstack/react-query'
import type { Thread } from '../api'

/** Shape every ['mail-threads', …] query stores. */
interface ThreadPage {
  threads:  Thread[]
  has_more: boolean
  cursor:   string | null
  total:    number | null
}

/** What `patchThreadCaches` hands back so a failed request can be undone. */
export type ThreadCacheSnapshot = readonly (readonly [unknown[], ThreadPage | undefined])[]

/**
 * Rewrites the cached thread lists in place so a status change shows up on the
 * next frame instead of after two round trips (the mutation, then the refetch
 * of a 50-row page plus the 200-row tab overview). The request still runs, and
 * the lists are still revalidated afterwards — this only removes the wait.
 *
 * Returning `null` from `patch` drops the row and decrements the total. Rows
 * are patched in EVERY cached list: correct for read/star/important, which do
 * not depend on the view, and near enough for archive — the row does leave the
 * inbox, and the background refetch puts it back in "All mail".
 */
export function patchThreadCaches(
  qc: QueryClient,
  ids: Iterable<string>,
  patch: (thread: Thread) => Thread | null,
): ThreadCacheSnapshot {
  const wanted = new Set(ids)
  const entries = qc.getQueriesData<ThreadPage>({ queryKey: ['mail-threads'] })
  const snapshot: (readonly [unknown[], ThreadPage | undefined])[] = []

  for (const [key, data] of entries) {
    snapshot.push([key as unknown[], data])
    if (!data?.threads?.length) continue

    let changed = 0
    const threads: Thread[] = []
    for (const thread of data.threads) {
      if (!wanted.has(thread.id)) { threads.push(thread); continue }
      const next = patch(thread)
      if (next !== thread) changed++
      if (next) threads.push(next)
    }
    if (!changed) continue

    const removed = data.threads.length - threads.length
    qc.setQueryData(key, {
      ...data,
      threads,
      total: data.total != null ? Math.max(0, data.total - removed) : data.total,
    })
  }
  return snapshot
}

/** Puts the lists back as they were — used when the request fails. */
export function restoreThreadCaches(qc: QueryClient, snapshot: ThreadCacheSnapshot) {
  for (const [key, data] of snapshot) qc.setQueryData(key, data)
}

// ── Ready-made patches ───────────────────────────────────────────────────────

export const markRead   = (read: boolean) => (t: Thread): Thread =>
  ({ ...t, unread_count: read ? 0 : Math.max(t.unread_count, 1) })

export const toggleStar      = (t: Thread): Thread => ({ ...t, is_starred: !t.is_starred })
export const toggleImportant = (t: Thread): Thread => ({ ...t, is_important: !t.is_important })
/** Archive, delete, spam, snooze, mute: the row leaves the list being shown. */
export const removeRow = (): null => null
