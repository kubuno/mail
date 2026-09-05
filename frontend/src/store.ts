import { create } from 'zustand'
import { EmailAccount, EmailAddress, Label } from './api'

export interface ComposeInitial {
  to:       EmailAddress[]
  cc:       EmailAddress[]
  bcc?:     EmailAddress[]
  subject:  string
  bodyHtml: string
  /** Pre-filled attachments (e.g. « send this file by email » from Drive). */
  attachments?: { filename: string; mime: string; content: string; size: number }[]
  /** Draft this compose is bound to: set when re-opening after "undo send" (so
   *  auto-save keeps updating the SAME draft instead of creating a duplicate)
   *  and when resuming a draft from the Drafts folder. */
  draftId?: string
  /** Thread this compose replies to, preserved when resuming a reply draft so
   *  sending it still threads correctly. */
  replyToId?: string
  /** Open the composer with OpenPGP Sign + Encrypt pre-armed (« Message chiffré »). */
  secure?:   boolean
  /** Open the composer in full-screen mode (used by the « Fenêtre séparée » pop-out). */
  fullscreen?: boolean
  /** Open the composer with the schedule-send menu already open (« Message programmé »). */
  schedule?: boolean
}

/** Seed handed to the header search's advanced panel when the user clicks
 *  « Recherche avancée » on a folder filter bar: it opens pre-filled with the
 *  chip values. Fields mirror MailFilterPanel's own `Filters`. */
export interface AdvancedSearchSeed {
  from?:      string
  to?:        string
  hasAttach?: boolean
  searchIn?:  string
  dateRange?: string
  customDate?: string | null
}

interface MailState {
  accounts:        EmailAccount[]
  selectedAccount: string | null
  currentFolder:   string
  currentLabelId:  string | null    // dossier virtuel « libellé » (route /mail/label/:id)
  /** Provider folder shown when currentFolder === 'custom' (route /mail/folder/:name). */
  currentImapFolder: string | null
  inboxCategory:   string           // catégorie active de la boîte de réception
  selectedThread:  string | null
  labels:          Label[]
  searchQuery:     string
  composeOpen:     boolean
  replyToId:       string | null
  composeInitial:  ComposeInitial | null
  splitMode:       'none' | 'vertical' | 'horizontal'
  density:         'comfortable' | 'compact'
  /** Compose to start once a thread opened from the list has loaded. */
  pendingCompose:  { threadId: string; mode: 'reply' | 'replyAll' | 'forward' } | null
  /** Set by a folder filter bar's « Recherche avancée » chip; the header search
   *  watches it, opens its advanced panel seeded with these values, then clears it. */
  advancedSearchSeed: AdvancedSearchSeed | null
  /** « Nouveau libellé » from the shell's New menu: a bumped nonce the sidebar
   *  watches to open its create-label dialog (the menu and the sidebar are
   *  separate components, so they talk through the store). */
  createLabelNonce: number
  /** « Nouveau filtre / règle »: a bumped nonce the header search bar watches to
   *  open its advanced-filter panel (which hosts the filter-creation flow). */
  newFilterNonce:  number
  /** « À partir d'un modèle » / « Enregistrer comme modèle »: the templates
   *  manager modal, mounted globally (MailComposeGlobal). */
  templatesOpen:   boolean
  /** « Nouvelle liste de diffusion »: the recipient-groups manager modal. */
  groupsOpen:      boolean

  setAccounts:       (accounts: EmailAccount[]) => void
  setComposeInitial: (d: ComposeInitial | null) => void
  setDensity:        (d: 'comfortable' | 'compact') => void
  setSelectedAccount:(id: string | null) => void
  setCurrentFolder:  (folder: string, labelId?: string | null, imapFolder?: string | null) => void
  setInboxCategory:  (cat: string) => void
  setSelectedThread: (id: string | null) => void
  setLabels:         (labels: Label[]) => void
  setSearchQuery:    (q: string) => void
  setComposeOpen:    (open: boolean, replyToId?: string | null) => void
  setSplitMode:      (mode: 'none' | 'vertical' | 'horizontal') => void
  setPendingCompose: (p: { threadId: string; mode: 'reply' | 'replyAll' | 'forward' } | null) => void
  setAdvancedSearchSeed: (s: AdvancedSearchSeed | null) => void
  requestCreateLabel: () => void
  requestNewFilter:   () => void
  setTemplatesOpen:   (open: boolean) => void
  setGroupsOpen:      (open: boolean) => void
}

const SPLIT_KEY = 'kubuno_mail_split'
function initialSplit(): 'none' | 'vertical' | 'horizontal' {
  const v = typeof localStorage !== 'undefined' ? localStorage.getItem(SPLIT_KEY) : null
  return v === 'vertical' || v === 'horizontal' ? v : 'none'
}
const DENSITY_KEY = 'kubuno_mail_density'
function initialDensity(): 'comfortable' | 'compact' {
  const v = typeof localStorage !== 'undefined' ? localStorage.getItem(DENSITY_KEY) : null
  return v === 'compact' ? 'compact' : 'comfortable'
}

export const useMailStore = create<MailState>((set) => ({
  accounts:        [],
  selectedAccount: null,
  currentFolder:   'inbox',
  currentLabelId:  null,
  currentImapFolder: null,
  inboxCategory:   'main',
  selectedThread:  null,
  labels:          [],
  searchQuery:     '',
  composeOpen:     false,
  replyToId:       null,
  composeInitial:  null,
  splitMode:       initialSplit(),
  density:         initialDensity(),
  pendingCompose:  null,
  advancedSearchSeed: null,
  createLabelNonce: 0,
  newFilterNonce:  0,
  templatesOpen:   false,
  groupsOpen:      false,

  setAccounts:        (accounts)       => set({ accounts }),
  requestCreateLabel: () => set(s => ({ createLabelNonce: s.createLabelNonce + 1 })),
  requestNewFilter:   () => set(s => ({ newFilterNonce: s.newFilterNonce + 1 })),
  setTemplatesOpen:   (templatesOpen)  => set({ templatesOpen }),
  setGroupsOpen:      (groupsOpen)     => set({ groupsOpen }),
  setAdvancedSearchSeed: (advancedSearchSeed) => set({ advancedSearchSeed }),
  setComposeInitial:  (composeInitial) => set({ composeInitial }),
  setPendingCompose:  (pendingCompose)  => set({ pendingCompose }),
  setDensity:         (density)        => { try { localStorage.setItem(DENSITY_KEY, density) } catch { /* ignore */ } set({ density }) },
  setSplitMode:       (mode)           => { try { localStorage.setItem(SPLIT_KEY, mode) } catch { /* ignore */ } set({ splitMode: mode }) },
  setSelectedAccount: (id)             => set({ selectedAccount: id, selectedThread: null }),
  // Navigating to a folder / label LEAVES the search (Gmail parity): the
  // committed query, like the open conversation, belongs to the view being
  // left. A scope inside a search is expressed in the query (`in:spam`), never
  // by the folder the sidebar highlights.
  setCurrentFolder:   (folder, labelId = null, imapFolder = null) =>
    set({ currentFolder: folder, currentLabelId: labelId, currentImapFolder: imapFolder, selectedThread: null, searchQuery: '' }),
  setInboxCategory:   (inboxCategory)  => set({ inboxCategory }),
  setSelectedThread:  (id)             => set({ selectedThread: id }),
  setLabels:          (labels)         => set({ labels }),
  setSearchQuery:     (searchQuery)    => set({ searchQuery }),
  setComposeOpen:     (open, replyToId = null) => set({ composeOpen: open, replyToId }),
}))
