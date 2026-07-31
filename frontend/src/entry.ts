/** Bundle MODULE mail — chargé à l'exécution (cf. vite.module.config). */
import { lazy } from 'react'
import { Inbox, Star, Send, FileText } from 'lucide-react'
import { RouteRegistry, WaffleAppRegistry, ModuleSettingsRegistry, NotificationRegistry, FaviconRegistry, SlotRegistry, useSidebarStore, useToolbarStore, useSearchStore, SDK_VERSION } from '@kubuno/sdk'
import './index.css'
import './i18n'
import { useMailStore } from './store'
import MailLogo from './MailLogo'
import MailSidebarBody from './MailSidebarBody'
import MailCreateMenu from './MailCreateMenu'
import MailSearchBar from './MailSearchBar'
import MailSendFileAction from './MailSendFileAction'
import MailComposeGlobal from './MailComposeGlobal'

export const sdkVersion = SDK_VERSION

export function register() {
  FaviconRegistry.register('mail', '/mail-logo.svg')

  // « Envoyer ce fichier par e-mail » dans le menu Partager de Drive (l'entrée
  // n'existe que si le module mail est actif) + composeur monté globalement
  // pour flotter au-dessus de n'importe quel module.
  SlotRegistry.register('files-share-actions', 'mail', MailSendFileAction)
  SlotRegistry.register('app-dialogs', 'mail', MailComposeGlobal)

  WaffleAppRegistry.register('mail', 'Mail', [
    { id: 'mail', label: 'Mail', Icon: MailLogo, path: '/mail' },
  ])

  useSidebarStore.getState().register({
    moduleId:    'mail',
    routePrefix: '/mail',
    SidebarBody: MailSidebarBody,
    collapsedBody: true,
    // Use the shell's default "New" button (multicolor +) instead of a bespoke
    // one inside the sidebar body; its dropdown offers "New message".
    newButtonLabelKey: 'mail:new_message',
    NewActions: MailCreateMenu,
    // Bottom nav (portrait) / left rail (landscape) rendered by the shell on
    // mobile — the main folders, with short labels (nav_* keys).
    mobileTabs: [
      { id: 'inbox',   labelKey: 'mail:nav_inbox',   Icon: Inbox,    path: '/mail', end: true },
      { id: 'starred', labelKey: 'mail:nav_starred', Icon: Star,     path: '/mail/starred' },
      { id: 'sent',    labelKey: 'mail:nav_sent',    Icon: Send,     path: '/mail/sent' },
      { id: 'drafts',  labelKey: 'mail:nav_drafts',  Icon: FileText, path: '/mail/drafts' },
    ],
  })

  useToolbarStore.getState().register({
    moduleId:    'mail',
    routePrefix: '/mail',
    noPadding:   true,
  })

  useSearchStore.getState().register({
    moduleId:    'mail',
    routePrefix: '/mail',
    placeholder: 'Rechercher dans les messages…',
    placeholderKey: 'mail:mail_search_ph',
    // Full replacement of the core bar: Gmail-style operators, ghost
    // completion, quick chips, live preview (the advanced-filter panel is
    // opened from inside MailSearchBar).
    SearchComponent: MailSearchBar,
    // Opt-out du système loupe → mode recherche : mail garde sa barre inline
    // permanente dans l'en-tête (façon Gmail).
    inline: true,
  })

  // The header gear button opens the per-user Mail settings while in /mail.
  ModuleSettingsRegistry.register('mail')

  // Declare the notification activities shown in the core Settings → Notifications matrix.
  NotificationRegistry.register({
    moduleId: 'mail',
    title: 'Mail',
    order: 30,
    activities: [
      { id: 'mail_received', label: 'Nouvel e-mail reçu', pushDefault: true },
      { id: 'mail_important', label: 'E-mail important reçu', emailDefault: true, pushDefault: true },
      { id: 'mail_spam', label: 'Un e-mail a été classé comme spam' },
    ],
  })

  // Routes
  const MailApp          = lazy(() => import('./MailApp'))
  const MailSettingsPage = lazy(() => import('./MailSettingsPage'))

  RouteRegistry.register('mail',           MailApp)
  RouteRegistry.register('mail/sent',      MailApp)
  RouteRegistry.register('mail/drafts',    MailApp)
  RouteRegistry.register('mail/starred',       MailApp)
  RouteRegistry.register('mail/snoozed',       MailApp)
  RouteRegistry.register('mail/important',     MailApp)
  RouteRegistry.register('mail/all',           MailApp)
  RouteRegistry.register('mail/scheduled',     MailApp)
  RouteRegistry.register('mail/spam',          MailApp)
  RouteRegistry.register('mail/trash',         MailApp)
  RouteRegistry.register('mail/subscriptions', MailApp)
  RouteRegistry.register('mail/label/:id',     MailApp)
  RouteRegistry.register('mail/settings',      MailSettingsPage)
}
