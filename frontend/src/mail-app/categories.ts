import { Inbox, Users, Info, Tag } from 'lucide-react'
import type { Thread } from '../api'

// ── Tabs ──────────────────────────────────────────────────────────────────────

export const TABS = [
  { id: 'main',          labelKey: 'mail_tab_primary',       Icon: Inbox, badge: 'bg-primary text-white' },
  { id: 'promotions',    labelKey: 'mail_tab_promotions',    Icon: Tag,   badge: 'bg-[#188038] text-white' },
  { id: 'social',        labelKey: 'mail_tab_social',        Icon: Users, badge: 'bg-[#1a73e8] text-white' },
  { id: 'notifications', labelKey: 'mail_tab_notifications', Icon: Info,  badge: 'bg-[#5f6368] text-white' },
] as const
export type MailCategory = typeof TABS[number]['id']

/**
 * Tab a conversation belongs to.
 *
 * The category is decided by the server when the message is stored (Rust
 * `services::categorize`) and simply read back here — the client no longer
 * classifies anything, so there is a single source of truth and listing a tab
 * costs an indexed lookup instead of a scan. `main` only covers rows stored
 * before the column existed.
 */
export function threadTab(t: Thread): MailCategory {
  return (t.category as MailCategory) || 'main'
}
