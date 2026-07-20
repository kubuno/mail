// Inbox categories live in the URL HASH rather than in a route of their own:
// /mail/#category/promotions. They are a client-side filter over the inbox, so
// they must not create server routes — but they still deserve a real, shareable
// link (the sidebar and the inbox tabs are anchors, never buttons).
import type { MailCategory } from './MailApp'

export const CATEGORY_IDS = ['main', 'promotions', 'social', 'notifications'] as const

/** `to=` value for a category link: /mail/#category/<id>. */
export function categoryTo(id: string): string {
  return `/mail/#category/${id}`
}

/** Category encoded in a location hash; 'main' when absent or unknown. */
export function categoryFromHash(hash: string): MailCategory {
  const id = /^#category\/([a-z]+)$/.exec(hash)?.[1]
  return (CATEGORY_IDS as readonly string[]).includes(id ?? '')
    ? (id as MailCategory)
    : 'main'
}
