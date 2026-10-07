// App-wide mount of the mail composer AND the compose-related dialogs
// (SlotRegistry 'app-dialogs'): the floating « Nouveau message » window and the
// templates / distribution-list managers can open above ANY module (e.g. from
// Drive's « send this file by email », or from the shell's New menu), not only
// inside /mail. Hosting the managers here keeps them alive after the New
// dropdown that opened them has closed.
import { lazy, Suspense } from 'react'
import { useMailStore } from './store'

const ComposeWindow  = lazy(() => import('./ComposeWindow'))
const TemplatesModal  = lazy(() => import('./MailTemplatesModal'))
const GroupsModal     = lazy(() => import('./MailGroupsModal'))

export default function MailComposeGlobal() {
  const composeOpen   = useMailStore(s => s.composeOpen)
  const templatesOpen = useMailStore(s => s.templatesOpen)
  const groupsOpen    = useMailStore(s => s.groupsOpen)
  const setTemplatesOpen = useMailStore(s => s.setTemplatesOpen)
  const setGroupsOpen    = useMailStore(s => s.setGroupsOpen)

  return (
    <Suspense fallback={null}>
      {composeOpen && <ComposeWindow />}
      {templatesOpen && <TemplatesModal onClose={() => setTemplatesOpen(false)} />}
      {groupsOpen && <GroupsModal onClose={() => setGroupsOpen(false)} />}
    </Suspense>
  )
}
