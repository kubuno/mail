import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useLocation } from 'react-router-dom'
import { useQuery } from '@tanstack/react-query'
import { Mail as MailIcon } from 'lucide-react'
import { useIsMobile } from '@ui'
import { mailApi } from './api'
import { useMailStore } from './store'
// ComposeWindow is mounted app-wide via the 'app-dialogs' slot (entry.ts) so
// the composer can float above any module, /mail included.
import PdfViewerModal from './PdfViewerModal'
import { DraftsView, ScheduledView, SubscriptionsView } from './MailViews'
import { useUndoSendStore } from './undoSendStore'
import { folderFromPath, plainKey } from './mail-app/helpers'
import ThreadList from './mail-app/ThreadList'
import ThreadReader from './mail-app/ThreadReader'
import UndoSendToast from './mail-app/UndoSendToast'
import SendErrorToast from './mail-app/SendErrorToast'

export type { MailCategory } from './mail-app/categories'

// ── Main MailApp ──────────────────────────────────────────────────────────────

export default function MailApp() {
  const { t } = useTranslation('mail')
  const { composeOpen, setComposeOpen, setComposeInitial, setAccounts, accounts, setCurrentFolder, currentFolder, selectedThread, splitMode } = useMailStore()
  // Mobile: always single pane (list ↔ reader via selectedThread) — a persisted
  // split mode from desktop makes no sense on a phone.
  const isMobile = useIsMobile()
  const effSplit = isMobile ? 'none' : splitMode
  const undoPayload = useUndoSendStore(s => s.payload)
  const cancelUndo  = useUndoSendStore(s => s.cancel)
  const sendError    = useUndoSendStore(s => s.error)
  const dismissError = useUndoSendStore(s => s.dismissError)

  // Global shortcut: "c" = new message.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!plainKey(e)) return
      if (e.key === 'c') { e.preventDefault(); setComposeOpen(true) }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [setComposeOpen])

  // « Fenêtre séparée » pop-out: opened via window.open('/mail?compose=1'). Boot
  // the full app with the composer auto-opened (maximized), then strip the param
  // so a reload doesn't reopen it. Draft auto-save works exactly as in-tab.
  useEffect(() => {
    const sp = new URLSearchParams(window.location.search)
    if (sp.get('compose') !== '1') return
    setComposeInitial({ to: [], cc: [], subject: '', bodyHtml: '', fullscreen: true })
    setComposeOpen(true)
    sp.delete('compose')
    const qs = sp.toString()
    window.history.replaceState(null, '', `${window.location.pathname}${qs ? `?${qs}` : ''}${window.location.hash}`)
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const { pathname } = useLocation()
  const [pdfUrl,  setPdfUrl]  = useState<string | null>(null)
  const [pdfName, setPdfName] = useState('')
  const openPdf = (url: string, name: string) => { setPdfUrl(url); setPdfName(name) }

  const { data: accountsData } = useQuery({
    queryKey: ['mail-accounts'],
    queryFn:  mailApi.listAccounts,
  })

  useEffect(() => {
    const labelMatch = pathname.match(/^\/mail\/label\/([^/]+)/)
    // A provider folder can contain slashes ("Projets/2026"), so its name takes
    // the whole tail of the path and travels URL-encoded.
    const folderMatch = pathname.match(/^\/mail\/folder\/(.+)$/)
    if (labelMatch)       setCurrentFolder('label', labelMatch[1])
    else if (folderMatch) setCurrentFolder('custom', null, decodeURIComponent(folderMatch[1]))
    else                  setCurrentFolder(folderFromPath(pathname))
  }, [pathname, setCurrentFolder])

  // The open conversation lives in the URL hash — «#inbox/<id>» — so a reload
  // or a shared link lands back on the same message. The older «?thread=<id>»
  // form still works: it is what "open in a new window" used to produce.
  useEffect(() => {
    // The thread id is always the LAST hash segment, whatever the base —
    // «#inbox/<id>» as well as «#category/promotions/<id>».
    const fromHash  = window.location.hash.match(/\/([0-9a-f-]{36})$/i)?.[1]
    const fromQuery = new URLSearchParams(window.location.search).get('thread')
    const id = fromHash ?? fromQuery
    if (id) useMailStore.getState().setSelectedThread(id)
  }, [])

  // Mirror the selection into the hash, keeping the current category segment.
  // ⚠️ Read the LIVE store value: on mount this effect runs in the same commit
  // as the deep-link one above, so the render-time value is still null and
  // would wipe the id straight out of the URL.
  const selectedForUrl = useMailStore(st => st.selectedThread)
  useEffect(() => {
    const live = useMailStore.getState().selectedThread ?? selectedForUrl
    // Strip only a trailing thread id: «#category/promotions» must survive
    // opening and closing a conversation (it carries the active tab).
    const base = window.location.hash.replace(/\/[0-9a-f-]{36}$/i, '') || '#inbox'
    const next = live ? `${base}/${live}` : base
    if (window.location.hash !== next) {
      window.history.replaceState(null, '', `${window.location.pathname}${window.location.search}${next}`)
    }
  }, [selectedForUrl])

  useEffect(() => {
    if (accountsData?.accounts) setAccounts(accountsData.accounts)
  }, [accountsData, setAccounts])

  const hasAccounts = accounts.length > 0

  if (!hasAccounts && accountsData !== undefined) {
    return (
      <div className="flex flex-col items-center justify-center h-full gap-4">
        <MailIcon size={48} className="text-text-tertiary opacity-40" />
        <div className="text-center">
          <p className="text-text-primary font-medium mb-1">{t('no_account')}</p>
          <p className="text-sm text-text-tertiary mb-4">
            {t('mail_no_account_hint')}
          </p>
          <a
            href="/mail/settings"
            className="inline-flex items-center h-9 px-4 text-sm font-medium rounded-md
                       bg-primary text-white hover:bg-primary-hover transition-colors"
          >
            {t('mail_configure_account')}
          </a>
        </div>
      </div>
    )
  }

  return (
    <div className="flex h-full overflow-hidden">
      {/* Folder navigation lives in the core shell's left panel now
          (MailSidebarBody registered via entry.ts, like every other module). */}

      {/* ── Main area ─────────────────────────────────────────────────────── */}
      {currentFolder === 'subscriptions' ? <SubscriptionsView />
        : currentFolder === 'drafts'     ? <DraftsView />
        : currentFolder === 'scheduled'  ? <ScheduledView />
        : effSplit === 'none'
          ? (
            // Single pane: the list stays MOUNTED and the reader overlays it,
            // rather than swapping one for the other. This preserves the list's
            // selection, scroll position and loaded pages when the user opens a
            // message and comes back — exactly as Gmail does.
            <div className="relative flex-1 min-w-0 flex flex-col overflow-hidden">
              <ThreadList />
              {selectedThread && (
                <div className="absolute inset-0 z-10 flex flex-col">
                  <ThreadReader onOpenPdf={openPdf} />
                </div>
              )}
            </div>
          )
          : (
            <div className={effSplit === 'vertical'
              ? 'flex flex-1 min-w-0 overflow-hidden'
              : 'flex flex-col flex-1 min-h-0 overflow-hidden'}>
              <div className={effSplit === 'vertical'
                ? 'w-[42%] min-w-[340px] max-w-[600px] border-r border-[#e0e0e0] flex flex-col overflow-hidden'
                : 'h-[45%] min-h-[200px] border-b border-[#e0e0e0] flex flex-col overflow-hidden'}>
                <ThreadList />
              </div>
              <div className="flex-1 min-w-0 min-h-0 overflow-hidden flex flex-col">
                {selectedThread
                  ? <ThreadReader onOpenPdf={openPdf} />
                  : (
                    <div className="flex-1 flex items-center justify-center text-text-tertiary text-sm bg-surface-1/30">
                      {t('split_pick', { defaultValue: 'Sélectionnez une conversation à lire' })}
                    </div>
                  )}
              </div>
            </div>
          )
      }

      {/* ComposeWindow: mounted globally through the app-dialogs slot (entry.ts). */}

      {/* "Undo send" countdown toast (same mechanics as drive's delete
          countdown: card + progress bar that empties). */}
      {undoPayload && (
        <UndoSendToast
          label={t('undo_sent', { defaultValue: 'Message envoyé' })}
          undoLabel={t('undo_cancel', { defaultValue: 'Annuler' })}
          onCancel={() => {
            const p = cancelUndo()
            if (p) {
              setComposeInitial({ to: p.to_addresses, cc: p.cc_addresses ?? [], subject: p.subject, bodyHtml: p.body_html, draftId: p.draft_id })
              setComposeOpen(true)
            }
          }}
        />
      )}
      {/* Delayed-send failure (fires after the compose window has closed). */}
      {sendError && <SendErrorToast message={sendError} onClose={dismissError} />}
      {pdfUrl && (
        <PdfViewerModal url={pdfUrl} filename={pdfName} onClose={() => setPdfUrl(null)} />
      )}
    </div>
  )
}
