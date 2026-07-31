// App-wide mount of the mail composer (SlotRegistry 'app-dialogs'): the
// floating « Nouveau message » window can open above ANY module (e.g. from
// Drive's « send this file by email »), not only inside /mail.
import { lazy, Suspense } from 'react'
import { useMailStore } from './store'

const ComposeWindow = lazy(() => import('./ComposeWindow'))

export default function MailComposeGlobal() {
  const open = useMailStore(s => s.composeOpen)
  if (!open) return null
  return <Suspense fallback={null}><ComposeWindow /></Suspense>
}
