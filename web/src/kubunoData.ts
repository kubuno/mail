/**
 * Cross-module data sharing (JSON envelopes over the clipboard) — thin typed
 * facade over the platform service published by the core at runtime
 * (`ModuleServiceRegistry('core', …)`). No vendored logic: once
 * `@kubuno/sdk >= 0.1.3` is published on npm, these can become direct SDK
 * imports; until then only the TYPES are declared locally.
 */
import { ModuleServiceRegistry } from '@kubuno/sdk'
import type React from 'react'

export interface KubunoDataEnvelope {
  kubuno: 1
  type: string
  module: string
  title?: string
  text?: string
  href?: string
  data: unknown
}

export interface DataCardProps { envelope: KubunoDataEnvelope }

export interface DataCardStaticRender {
  svg?: string
  dataUrl?: string
  width: number
  height: number
}

export interface DataCardRenderer {
  types: string[]
  Component?: React.ComponentType<DataCardProps>
  renderStatic?: (envelope: KubunoDataEnvelope) => Promise<DataCardStaticRender | null>
}

/** Writes an envelope to the system clipboard (dual text/plain + text/html). */
export function copyKubunoData(envelope: KubunoDataEnvelope): Promise<boolean> {
  return ModuleServiceRegistry.call<Promise<boolean>>('core', 'copyKubunoData', envelope) ?? Promise.resolve(false)
}

/** Extracts an envelope from a paste/drop DataTransfer, if any. */
export function readKubunoData(dt: DataTransfer | null): KubunoDataEnvelope | null {
  return ModuleServiceRegistry.call<KubunoDataEnvelope | null>('core', 'readKubunoData', dt) ?? null
}

/** Registers this module's card renderer on the `core.data-card` extension point. */
export function registerDataCardRenderer(moduleId: string, renderer: DataCardRenderer): void {
  ModuleServiceRegistry.call('core', 'registerDataCardRenderer', moduleId, renderer)
}

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
}

/**
 * Semantic HTML block for a pasted envelope. Deliberately restricted to
 * blockquote/b/br/a with an absolute http(s) href: the backend passes outgoing
 * bodies through ammonia's default sanitizer, which strips styles, data-*
 * attributes and exotic tags — this structure survives it end-to-end.
 */
export function kubunoDataToEmailHtml(env: KubunoDataEnvelope): string {
  const parts: string[] = []
  if (env.title) parts.push(`<b>${escapeHtml(env.title)}</b>`)
  if (env.text) {
    for (const l of env.text.split('\n')) {
      // Skip a first text line that just repeats the title.
      if (parts.length === 1 && l.trim() === env.title) continue
      parts.push(escapeHtml(l))
    }
  }
  if (env.href) {
    const url = `${location.origin}${env.href}`
    parts.push(`<a href="${escapeHtml(url)}">${escapeHtml(url)}</a>`)
  }
  if (!parts.length) parts.push(escapeHtml(env.type))
  return `<blockquote>${parts.join('<br>')}</blockquote><br>`
}
