// Per-user mail preferences, kept in localStorage. Extracted from the settings
// page so a reader component can consult one preference without pulling the
// whole settings UI into its bundle.
export interface VacationResponder {
  enabled:      boolean
  startDate:    string   // YYYY-MM-DD, first day the responder is active
  endDate:      string   // YYYY-MM-DD, last day ('' = no end date)
  subject:      string
  messageHtml:  string   // rich-text body (RichTextEditor)
  contactsOnly: boolean  // reply only to people in the user's contacts
}

export interface MailPrefs {
  // Reading & list
  pageSize:          string   // conversations per page
  previews:          string   // 'preview' (snippet) | 'subject'
  personalLevel:     string   // personal-level indicators: 'none' | 'show'
  markAsRead:        string   // reading-pane mark-as-read: 'immediately' | '1' | '3' | '5' | 'never'
  conversationView:  boolean  // group messages by thread
  // Composing & sending
  undoDelay:         string   // undo-send window in seconds ('0' disables)
  defaultReply:      string   // 'reply' | 'reply_all'
  sendAndArchive:    boolean   // show a "Send & archive" button in the composer
  smartReply:        boolean   // suggested one-tap replies
  nudgesReply:       boolean   // nudge to reply to messages that may need one
  nudgesFollowup:    boolean   // nudge to follow up on messages awaiting an answer
  // Writing help
  grammar:           boolean
  spelling:          boolean
  autocorrect:       boolean
  // Default text style (body of new messages)
  defaultFont:       string   // family
  defaultSize:       string   // px
  defaultColor:      string   // hex
  // Interface
  showImages:        string   // 'always' | 'ask'
  hoverActions:      boolean   // quick actions on row hover
  keyboardShortcuts: boolean
  buttonLabels:      string   // toolbar buttons: 'icons' | 'text'
  desktopNotifications: string // 'off' | 'all' | 'important'
  createContacts:    boolean   // create contacts for auto-complete from sent mail
  // Vacation responder (auto-reply)
  vacation:          VacationResponder
  // Signature defaults (which signature to prefill)
  signatureNew:      string   // signature id used for new mails, '' = none
  signatureReply:    string   // signature id used for replies/forwards, '' = none
}

export const DEFAULT_TEXT_STYLE = { font: 'Arial', size: '13', color: '#202124' }

export const DEFAULT_PREFS: MailPrefs = {
  pageSize: '25', previews: 'preview', personalLevel: 'none',
  markAsRead: 'immediately', conversationView: true,
  undoDelay: '5', defaultReply: 'reply', sendAndArchive: false,
  smartReply: true, nudgesReply: true, nudgesFollowup: true,
  grammar: true, spelling: true, autocorrect: true,
  defaultFont: DEFAULT_TEXT_STYLE.font, defaultSize: DEFAULT_TEXT_STYLE.size, defaultColor: DEFAULT_TEXT_STYLE.color,
  showImages: 'ask', hoverActions: true, keyboardShortcuts: false,
  buttonLabels: 'icons', desktopNotifications: 'off', createContacts: true,
  vacation: { enabled: false, startDate: '', endDate: '', subject: '', messageHtml: '', contactsOnly: false },
  signatureNew: '', signatureReply: '',
}

export function loadPrefs(): MailPrefs {
  try {
    const s = localStorage.getItem('mail-prefs')
    if (s) return { ...DEFAULT_PREFS, ...JSON.parse(s) }
  } catch { /* ignore */ }
  return DEFAULT_PREFS
}
