import { create } from 'zustand'
import { EmailAccount, EmailAddress, Label } from './api'

export interface ComposeInitial {
  to:       EmailAddress[]
  cc:       EmailAddress[]
  subject:  string
  bodyHtml: string
}

interface MailState {
  accounts:        EmailAccount[]
  selectedAccount: string | null
  currentFolder:   string
  currentLabelId:  string | null    // dossier virtuel « libellé » (route /mail/label/:id)
  inboxCategory:   string           // catégorie active de la boîte de réception
  selectedThread:  string | null
  labels:          Label[]
  searchQuery:     string
  composeOpen:     boolean
  replyToId:       string | null
  composeInitial:  ComposeInitial | null
  splitMode:       'none' | 'vertical' | 'horizontal'
  density:         'comfortable' | 'compact'

  setAccounts:       (accounts: EmailAccount[]) => void
  setComposeInitial: (d: ComposeInitial | null) => void
  setDensity:        (d: 'comfortable' | 'compact') => void
  setSelectedAccount:(id: string | null) => void
  setCurrentFolder:  (folder: string, labelId?: string | null) => void
  setInboxCategory:  (cat: string) => void
  setSelectedThread: (id: string | null) => void
  setLabels:         (labels: Label[]) => void
  setSearchQuery:    (q: string) => void
  setComposeOpen:    (open: boolean, replyToId?: string | null) => void
  setSplitMode:      (mode: 'none' | 'vertical' | 'horizontal') => void
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
  inboxCategory:   'principale',
  selectedThread:  null,
  labels:          [],
  searchQuery:     '',
  composeOpen:     false,
  replyToId:       null,
  composeInitial:  null,
  splitMode:       initialSplit(),
  density:         initialDensity(),

  setAccounts:        (accounts)       => set({ accounts }),
  setComposeInitial:  (composeInitial) => set({ composeInitial }),
  setDensity:         (density)        => { try { localStorage.setItem(DENSITY_KEY, density) } catch { /* ignore */ } set({ density }) },
  setSplitMode:       (mode)           => { try { localStorage.setItem(SPLIT_KEY, mode) } catch { /* ignore */ } set({ splitMode: mode }) },
  setSelectedAccount: (id)             => set({ selectedAccount: id, selectedThread: null }),
  setCurrentFolder:   (folder, labelId = null) => set({ currentFolder: folder, currentLabelId: labelId, selectedThread: null }),
  setInboxCategory:   (inboxCategory)  => set({ inboxCategory }),
  setSelectedThread:  (id)             => set({ selectedThread: id }),
  setLabels:          (labels)         => set({ labels }),
  setSearchQuery:     (searchQuery)    => set({ searchQuery }),
  setComposeOpen:     (open, replyToId = null) => set({ composeOpen: open, replyToId }),
}))
