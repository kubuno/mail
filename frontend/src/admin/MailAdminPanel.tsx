/**
 * What the mail module contributes to its OWN admin page, and WHERE.
 *
 * ── Placed, not stacked ──────────────────────────────────────────────────────
 * The panel used to be one page: state, diagnostic, keys, then forty-eight
 * settings in ten folds. It is now split into the five pages this module
 * declares in `module.toml` (`[[setting_groups]]`), each a menu entry and an
 * address of its own — so these two views have to say which page they belong to,
 * in the module's OWN vocabulary. The core places them from that and knows
 * nothing else about mail.
 *
 *  • The diagnostic goes to `overview`, WITHOUT a tab: it is what an operator
 *    opens the page for, and what tells them which setting to go and change. A
 *    page whose only job is "is anything broken" is the right place for it, and
 *    a tab would be one click between the operator and the answer.
 *  • The DKIM keys go to `authentication`, WITH a tab, beside the categories of
 *    that page. Signing keys are not a setting — nothing is typed, a key is
 *    generated, published, then rotated — so they earn their own tab rather
 *    than being wedged above someone else's form.
 *
 * Both are split out of the module's main bundle: this is admin-only, and the
 * mail application itself must not pay for it on every load.
 */
import { lazy, Suspense, type ComponentType } from 'react'
import { ModuleAdminRegistry } from '@kubuno/sdk'
import { Spinner } from '@ui'
import { registerDomainDiagnostics } from './domainDiagnostics'

const DnsSetupSection    = lazy(() => import('./DnsSetupSection'))
const DiagnosticsSection = lazy(() => import('./DiagnosticsSection'))
const DkimSection        = lazy(() => import('./DkimSection'))
const RelaySection       = lazy(() => import('./RelaySection'))
const AddressesSection   = lazy(() => import('./addresses/AddressesSection'))

/** Keeps the fallback identical between the two, so a page does not jump. */
function deferred(Section: ComponentType): ComponentType {
  return function Deferred() {
    return (
      <Suspense fallback={<div className="flex justify-center py-8"><Spinner /></div>}>
        <Section />
      </Suspense>
    )
  }
}

/** Declares both sections. Called once, from the module's `register()`. */
export function registerMailAdmin() {
  // Instance ▸ Domaines asks "what does this domain's DNS say?"; this module is
  // the one that knows. Registered here rather than in a lazy chunk, because the
  // core reads the registry as it renders the domain sheet.
  registerDomainDiagnostics()

  // DNS setup goes FIRST on the overview: "here is what to publish" precedes
  // "here is what is wrong". No label, so it renders inline like the diagnostic
  // rather than hiding the records one click away behind a tab.
  ModuleAdminRegistry.register({
    moduleId:  'mail',
    id:        'dns-setup',
    group:     'overview',
    position:  5,
    Component: deferred(DnsSetupSection),
  })

  ModuleAdminRegistry.register({
    moduleId:  'mail',
    id:        'diagnostics',
    group:     'overview',
    // No label: rendered inline at the top of the page rather than as a tab.
    position:  10,
    Component: deferred(DiagnosticsSection),
  })

  // The `addresses` page declares no setting — it manages OBJECTS. So the
  // section is registered WITHOUT a label: it is not one tab beside others, it
  // is the whole page, and it brings its own four tabs (boxes, aliases, lists,
  // domains) because those four are one question asked four ways.
  ModuleAdminRegistry.register({
    moduleId:  'mail',
    id:        'addresses',
    group:     'addresses',
    position:  10,
    Component: deferred(AddressesSection),
  })

  ModuleAdminRegistry.register({
    moduleId:  'mail',
    id:        'dkim',
    group:     'authentication',
    labelKey:  'mail:admin_dkim_tab',
    label:     'Clés DKIM',
    icon:      'KeyRound',
    position:  20,
    Component: deferred(DkimSection),
  })

  // The outbound relay goes to `authentication` too — every outbound setting
  // (`outbound_enabled`, DKIM signing, TLS level) and the DKIM keys already live
  // on that page, so "how is remote mail sent" is answered in one place. Its own
  // tab: it is not a settings form declared in `module.toml` but a small object
  // with a write-only secret, like the keys beside it.
  ModuleAdminRegistry.register({
    moduleId:  'mail',
    id:        'relay',
    group:     'authentication',
    labelKey:  'mail:admin_relay_tab',
    label:     'Relais sortant',
    icon:      'Server',
    position:  30,
    Component: deferred(RelaySection),
  })
}
