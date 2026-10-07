/**
 * The parts of `@kubuno/sdk` this module uses that are LIVE in the host but not
 * yet in the published type surface: `ModuleAdminRegistry` and `SearchConfig.inline`.
 *
 * The registry EXISTS at runtime already: `@kubuno/sdk` is `external`, the host
 * resolves it through its import map to its own instance, and that instance is
 * the core's — which exports this registry today. What the installed npm package
 * carries is only the type surface, and it is one release behind.
 *
 * So this is a declaration of an API that is live, not a shim around a missing
 * one. Delete this file once `@kubuno/sdk` is bumped past the release that adds
 * both; the imports it covers need no change.
 *
 * (Same reason `entry.ts` spelt `'module-admin:mail'` by hand rather than using
 * the SDK's `moduleAdminSlot()` helper.)
 */
import type { ComponentType } from 'react'

declare module '@kubuno/sdk' {
  /** One view a module contributes to its own admin page, and where it lives. */
  export interface ModuleAdminSection {
    /** The contributing module — its page is `/admin/modules/<moduleId>`. */
    moduleId:   string
    /** Stable, untranslated id, unique per module. Also the tab id. */
    id:         string
    /** A `[[setting_groups]]` id of the same module. Absent = its first page. */
    group?:     string
    /** Translated tab label. Absent = rendered inline, above the tabs. */
    label?:     string
    /** i18n key of the label, resolved at render so it follows the language. */
    labelKey?:  string
    /** Lucide icon name for the tab; an unknown name simply shows none. */
    icon?:      string
    /** Order among the contributed items of the same page (lower first). */
    position?:  number
    Component:  ComponentType
  }

  export const ModuleAdminRegistry: {
    register(section: ModuleAdminSection): void
    sectionsFor(moduleId: string): ModuleAdminSection[]
  }

  /**
   * Merged into the SDK's own `SearchConfig`: mail opts out of the magnifier →
   * search-mode behaviour and keeps its permanent inline bar in the header. The
   * host honours the flag today; only the published `.d.ts` is behind.
   */
  interface SearchConfig {
    inline?: boolean
  }
}
