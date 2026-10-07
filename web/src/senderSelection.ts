// Pure "From" (sender) selection logic, shared by the new-message composer and
// the inline reply/forward composer. Kept free of React so the rules — only
// active accounts may send, and a reply defaults to the account that received
// the message — live in one place and stay easy to reason about.

/** The minimal shape both composers need from an account. */
export interface SenderAccount {
  id:         string
  is_active:  boolean
  is_default: boolean
}

/** Accounts that may be picked as a sender: active ones only. */
export function activeSenders<A extends SenderAccount>(accounts: A[]): A[] {
  return accounts.filter(a => a.is_active)
}

/**
 * Default sender for a brand-new message: the default account among the active
 * ones, else the first active account, else undefined (no active account).
 */
export function defaultComposeSender<A extends SenderAccount>(accounts: A[]): A | undefined {
  const active = activeSenders(accounts)
  return active.find(a => a.is_default) ?? active[0]
}

/**
 * Default sender for a reply/forward: the account that RECEIVED the message
 * when it is still active, so a reply to mail delivered to address X leaves
 * from X — else the ordinary compose default (default active, then first
 * active).
 */
export function defaultReplySender<A extends SenderAccount>(
  accounts: A[],
  receivedAccountId: string | null | undefined,
): A | undefined {
  const active = activeSenders(accounts)
  return active.find(a => a.id === receivedAccountId) ?? defaultComposeSender(accounts)
}
