/** Bundle MODULE mail — chargé à l'exécution (cf. vite.module.config). */
import { lazy } from 'react'
import { RouteRegistry, WaffleAppRegistry, ModuleSettingsRegistry, NotificationRegistry, FaviconRegistry, useSidebarStore, useToolbarStore, useSearchStore, SDK_VERSION } from '@kubuno/sdk'
import './index.css'
import './i18n'
import { useMailStore } from './store'
import MailLogo from './MailLogo'
import MailSidebarBody from './MailSidebarBody'
import MailFilterPanel from './MailFilterPanel'

export const sdkVersion = SDK_VERSION

export function register() {
  FaviconRegistry.register('mail', '/mail-logo.svg')

  WaffleAppRegistry.register('mail', 'Mail', [
    { id: 'mail', label: 'Mail', Icon: MailLogo, path: '/mail' },
  ])

  useSidebarStore.getState().register({
    moduleId:    'mail',
    routePrefix: '/mail',
    SidebarBody: MailSidebarBody,
    collapsedBody: true,
    hideSidebar: true,
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
    onSearch:    (q) => useMailStore.getState().setSearchQuery(q),
    FilterPanel: MailFilterPanel,
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
