import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useQuery } from '@tanstack/react-query'
import { Plus, Trash2, Check, Palette, RotateCcw } from 'lucide-react'
import { Button, Input, Callout, FontSizeField, Checkbox, Dropdown } from '@ui'
import { SettingsRow, RadioGroup } from './SettingsRow'
import { RichTextEditor, MAIL_FONTS, MAIL_SIZES, MAIL_COLORS } from './RichTextEditor'
import { mailApi, apiErrorMessage } from '../api'
import { activeSenders } from '../senderSelection'
import {
  loadSignatures, saveSignatures, defaultSignatureId, setDefaultSignatureId,
  newSignatureId, loadSignatureDefaults, saveSignatureDefaults,
  type Signature, type SignatureDefaults, type SignatureKind,
} from '../signatures'

// Sentinel select value = « inherit the global default » (no per-address entry),
// distinct from '' which is a deliberate « no signature » for that address.
const INHERIT = '__inherit__'

// ── Preferences (stored client-side) ──────────────────────────────────────────

/**
 * Vacation responder (out-of-office auto-reply). Persisted server-side
 * (`GET`/`PUT /mail/vacation`) and actually sent on incoming mail by the mail
 * service, under the RFC 3834 anti-loop guard. Loaded from the server on mount
 * and saved there alongside the rest of these preferences.
 */
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
  showImages: 'always', hoverActions: true, keyboardShortcuts: false,
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

// ── Small building blocks ─────────────────────────────────────────────────────

/** Section title separating groups of related settings. */
function Section({ id, title }: { id?: string; title: string }) {
  return (
    <h3 id={id} className="text-sm font-medium text-[#202124] mt-8 mb-1 pt-6 border-t border-[#e8eaed] first:mt-0 first:pt-0 first:border-0 scroll-mt-4">
      {title}
    </h3>
  )
}

/** A single on/off checkbox control — thin wrapper over the @ui Checkbox primitive. */
function CheckboxControl({ checked, onChange, label }: {
  checked: boolean; onChange: (v: boolean) => void; label: string
}) {
  return <Checkbox checked={checked} onChange={onChange} label={label} />
}

/**
 * "Default text style": font family + size (FontSizeField) and a colour swatch,
 * with a live preview and a reset button — Gmail's "Default text style" control.
 */
function DefaultTextStyle({ font, size, color, onChange, onReset }: {
  font: string; size: string; color: string
  onChange: (patch: { font?: string; size?: string; color?: string }) => void
  onReset: () => void
}) {
  const { t } = useTranslation('mail')
  const [colorOpen, setColorOpen] = useState(false)
  return (
    <div className="space-y-3">
      <div className="flex items-center gap-2 flex-wrap">
        <FontSizeField
          font={font} onFontChange={v => onChange({ font: v })} fonts={MAIL_FONTS}
          size={size} onSizeChange={v => onChange({ size: v })} sizes={MAIL_SIZES}
          minSize={6} maxSize={96} height={30} fontWidth={140} sizeWidth={64} fontSize={14}
        />
        {/* Colour swatch + popover */}
        <div className="relative">
          <button
            type="button"
            onClick={() => setColorOpen(v => !v)}
            className="h-[30px] px-2 flex items-center gap-1.5 rounded-lg border border-border hover:bg-surface-1 text-sm text-text-primary"
            title={t('mail_text_color', { defaultValue: 'Couleur du texte' })}
          >
            <Palette size={14} />
            <span className="w-4 h-4 rounded-full border border-border" style={{ background: color }} />
          </button>
          {colorOpen && (
            <>
              <div className="fixed inset-0 z-40" onClick={() => setColorOpen(false)} />
              <div className="absolute top-full mt-1 left-0 z-50 bg-white border border-border rounded-lg shadow-lg p-2 grid grid-cols-4 gap-1.5 w-40">
                {MAIL_COLORS.map(c => (
                  <button
                    key={c} type="button"
                    onClick={() => { onChange({ color: c }); setColorOpen(false) }}
                    className="w-7 h-7 rounded-full border border-border" style={{ background: c }} title={c}
                  />
                ))}
              </div>
            </>
          )}
        </div>
        <Button variant="ghost" icon={<RotateCcw size={14} />} onClick={onReset}>
          {t('mail_default_style_reset', { defaultValue: 'Réinitialiser' })}
        </Button>
      </div>
      <div
        className="rounded-lg border border-border bg-surface-0 px-4 py-3"
        style={{ fontFamily: font, fontSize: `${size}px`, color }}
      >
        {t('mail_default_style_preview', { defaultValue: 'Voici à quoi ressemblera le corps de votre texte.' })}
      </div>
    </div>
  )
}

// ── General tab ───────────────────────────────────────────────────────────────

export function GeneralTab() {
  const { t } = useTranslation('mail')
  const [prefs, setPrefs] = useState<MailPrefs>(loadPrefs)
  const [saved, setSaved] = useState(false)
  const [vacError, setVacError] = useState('')

  // The vacation responder is the one preference that lives on the server (it
  // has to, so incoming mail can be answered while the user is offline). Load it
  // on mount and merge it over whatever localStorage held.
  useEffect(() => {
    let cancelled = false
    mailApi.getVacation()
      .then(v => {
        if (cancelled) return
        setPrefs(p => ({ ...p, vacation: {
          enabled: v.enabled, startDate: v.startDate, endDate: v.endDate,
          subject: v.subject, messageHtml: v.messageHtml, contactsOnly: v.contactsOnly,
        } }))
      })
      .catch(() => { /* keep the localStorage value if the server is unreachable */ })
    return () => { cancelled = true }
  }, [])

  // Signatures live in their own store, but are managed from this same tab.
  const [sigList, setSigList] = useState<Signature[]>(loadSignatures)
  const [defSig, setDefSig]   = useState<string>(defaultSignatureId)
  // Per-sending-address signature defaults (Gmail parity).
  const [sigDefaults, setSigDefaults] = useState<SignatureDefaults>(loadSignatureDefaults)

  // Possible sending addresses: active accounts + verified « send as » identities.
  // Both are read-only here; a missing send-as endpoint degrades to accounts only.
  const { data: accountsData } = useQuery({ queryKey: ['mail-accounts'], queryFn: mailApi.listAccounts })
  const { data: sendAsData }   = useQuery({ queryKey: ['mail-send-as'], queryFn: mailApi.listSendAs })
  const senderAddresses = (() => {
    const seen = new Set<string>()
    const out: { email: string; label: string }[] = []
    for (const a of activeSenders(accountsData?.accounts ?? [])) {
      const key = a.email_address.toLowerCase()
      if (seen.has(key)) continue
      seen.add(key)
      out.push({ email: a.email_address, label: `${a.name} <${a.email_address}>` })
    }
    for (const s of (sendAsData ?? []).filter(s => s.verified)) {
      const key = s.email.toLowerCase()
      if (seen.has(key)) continue
      seen.add(key)
      out.push({ email: s.email, label: s.displayName ? `${s.displayName} <${s.email}>` : s.email })
    }
    return out
  })()

  const perAddrValue = (email: string, kind: SignatureKind): string => {
    const entry = sigDefaults[email.toLowerCase()]
    if (entry && kind in entry) return entry[kind] ?? ''
    return INHERIT
  }
  const setPerAddr = (email: string, kind: SignatureKind, value: string) =>
    setSigDefaults(prev => {
      const key = email.toLowerCase()
      const entry = { ...(prev[key] ?? {}) }
      if (value === INHERIT) delete entry[kind]
      else entry[kind] = value
      const next = { ...prev, [key]: entry }
      if (entry.new === undefined && entry.reply === undefined) delete next[key]
      return next
    })

  const set = <K extends keyof MailPrefs>(key: K, value: MailPrefs[K]) =>
    setPrefs(p => ({ ...p, [key]: value }))

  // Patch a single field of the nested vacation-responder config.
  const setVac = <K extends keyof VacationResponder>(key: K, value: VacationResponder[K]) =>
    setPrefs(p => ({ ...p, vacation: { ...p.vacation, [key]: value } }))

  const updateSig = (id: string, patch: Partial<Signature>) =>
    setSigList(l => l.map(s => (s.id === id ? { ...s, ...patch } : s)))
  const addSig = () =>
    setSigList(l => [...l, { id: newSignatureId(), name: t('mail_sig_new', { defaultValue: 'Nouvelle signature' }), html: '' }])
  const removeSig = (id: string) => {
    setSigList(l => l.filter(s => s.id !== id))
    if (defSig === id) setDefSig('')
    // Also drop it from the signature defaults if it was selected there.
    setPrefs(p => ({
      ...p,
      signatureNew:   p.signatureNew === id ? '' : p.signatureNew,
      signatureReply: p.signatureReply === id ? '' : p.signatureReply,
    }))
    // Also drop it from any per-address default that pointed at it.
    setSigDefaults(prev => {
      const next: SignatureDefaults = {}
      for (const [addr, entry] of Object.entries(prev)) {
        const e = { ...entry }
        if (e.new === id) delete e.new
        if (e.reply === id) delete e.reply
        if (e.new !== undefined || e.reply !== undefined) next[addr] = e
      }
      return next
    })
  }

  const save = async () => {
    setVacError('')
    // Persist the vacation responder to the server first: it can be rejected
    // (an enabled responder needs a start day and a message), and a rejected
    // config must not be reported as saved.
    try {
      await mailApi.saveVacation(prefs.vacation)
    } catch (e) {
      setVacError(apiErrorMessage(e, t('mail_settings_vacation_save_error', {
        defaultValue: "La réponse automatique n'a pas pu être enregistrée.",
      })))
      return
    }
    localStorage.setItem('mail-prefs', JSON.stringify(prefs))
    saveSignatures(sigList)
    setDefaultSignatureId(defSig)
    saveSignatureDefaults(sigDefaults)
    setSaved(true)
    setTimeout(() => setSaved(false), 2500)
  }

  // Options for the "signature defaults" dropdowns: every signature + "None".
  const sigOptions = [
    { value: '', label: t('mail_sig_none', { defaultValue: 'Aucune signature' }) },
    ...sigList.map(s => ({ value: s.id, label: s.name || t('mail_sig_unnamed', { defaultValue: '(sans nom)' }) })),
  ]
  // Per-address dropdowns add an "inherit the global default" sentinel on top.
  const perAddrOptions = [
    { value: INHERIT, label: t('mail_settings_sig_inherit', { defaultValue: 'Valeur par défaut' }) },
    ...sigOptions,
  ]

  return (
    <div>
      {/* ── Reading & list ─────────────────────────────────────────────── */}
      <Section title={t('mail_settings_section_reading', { defaultValue: 'Lecture' })} />

      <SettingsRow label={t('mail_settings_page_size')} description={t('mail_settings_page_size_desc')}>
        <RadioGroup
          value={prefs.pageSize}
          onChange={v => set('pageSize', v)}
          options={[
            { value: '10',  label: t('mail_settings_convs_per_page', { count: 10 }) },
            { value: '25',  label: t('mail_settings_convs_per_page', { count: 25 }) },
            { value: '50',  label: t('mail_settings_convs_per_page', { count: 50 }) },
            { value: '100', label: t('mail_settings_convs_per_page', { count: 100 }) },
          ]}
        />
      </SettingsRow>

      <SettingsRow
        label={t('mail_settings_previews', { defaultValue: 'Aperçus' })}
        description={t('mail_settings_previews_desc', { defaultValue: "Afficher un extrait du message dans la liste, ou l'objet seul." })}
      >
        <RadioGroup
          value={prefs.previews}
          onChange={v => set('previews', v)}
          options={[
            { value: 'preview', label: t('mail_settings_previews_show', { defaultValue: 'Afficher un aperçu' }) },
            { value: 'subject', label: t('mail_settings_previews_subject', { defaultValue: 'Objet seul' }) },
          ]}
        />
      </SettingsRow>

      <SettingsRow
        label={t('mail_settings_personal_level', { defaultValue: 'Indicateurs de message personnel' })}
        description={t('mail_settings_personal_level_desc', { defaultValue: 'Marquer les messages qui vous sont adressés directement.' })}
      >
        <RadioGroup
          value={prefs.personalLevel}
          onChange={v => set('personalLevel', v)}
          options={[
            { value: 'none', label: t('mail_settings_personal_level_none', { defaultValue: 'Aucun indicateur' }) },
            { value: 'show', label: t('mail_settings_personal_level_show', { defaultValue: 'Afficher les indicateurs' }) },
          ]}
        />
      </SettingsRow>

      <SettingsRow
        label={t('mail_settings_mark_read', { defaultValue: "Volet d'aperçu — marquer comme lu" })}
        description={t('mail_settings_mark_read_desc', { defaultValue: "Quand un message est marqué comme lu à l'ouverture dans le volet." })}
      >
        <RadioGroup
          value={prefs.markAsRead}
          onChange={v => set('markAsRead', v)}
          options={[
            { value: 'immediately', label: t('mail_settings_mark_read_now', { defaultValue: 'Immédiatement' }) },
            { value: '1',           label: t('mail_settings_mark_read_after', { count: 1, defaultValue: 'Au bout de 1 seconde' }) },
            { value: '3',           label: t('mail_settings_mark_read_after', { count: 3, defaultValue: 'Au bout de 3 secondes' }) },
            { value: '5',           label: t('mail_settings_mark_read_after', { count: 5, defaultValue: 'Au bout de 5 secondes' }) },
            { value: 'never',       label: t('mail_settings_mark_read_never', { defaultValue: 'Jamais (marquer manuellement)' }) },
          ]}
        />
      </SettingsRow>

      <SettingsRow
        label={t('mail_settings_conversation_mode')}
        description={t('mail_settings_conversation_mode_desc')}
      >
        <CheckboxControl
          checked={prefs.conversationView}
          onChange={v => set('conversationView', v)}
          label={t('mail_settings_conversation_mode_on')}
        />
      </SettingsRow>

      {/* ── Composing & sending ────────────────────────────────────────── */}
      <Section title={t('mail_settings_section_sending', { defaultValue: 'Rédaction et envoi' })} />

      <SettingsRow
        label={t('mail_settings_undo_send')}
        description={t('mail_settings_undo_send_desc')}
      >
        <RadioGroup
          value={prefs.undoDelay}
          onChange={v => set('undoDelay', v)}
          options={[
            { value: '0',  label: t('mail_settings_disable') },
            { value: '5',  label: t('mail_settings_seconds', { count: 5 }) },
            { value: '10', label: t('mail_settings_seconds', { count: 10 }) },
            { value: '20', label: t('mail_settings_seconds', { count: 20 }) },
            { value: '30', label: t('mail_settings_seconds', { count: 30 }) },
          ]}
        />
      </SettingsRow>

      <SettingsRow label={t('mail_settings_default_reply')}>
        <RadioGroup
          value={prefs.defaultReply}
          onChange={v => set('defaultReply', v)}
          options={[
            { value: 'reply',     label: t('mail_settings_reply') },
            { value: 'reply_all', label: t('mail_settings_reply_all') },
          ]}
        />
      </SettingsRow>

      <SettingsRow
        label={t('mail_settings_send_archive', { defaultValue: 'Envoyer et archiver' })}
        description={t('mail_settings_send_archive_desc', { defaultValue: 'Afficher un bouton « Envoyer et archiver » dans les réponses.' })}
      >
        <RadioGroup
          value={prefs.sendAndArchive ? 'show' : 'hide'}
          onChange={v => set('sendAndArchive', v === 'show')}
          options={[
            { value: 'show', label: t('mail_settings_send_archive_show', { defaultValue: 'Afficher le bouton' }) },
            { value: 'hide', label: t('mail_settings_send_archive_hide', { defaultValue: 'Masquer le bouton' }) },
          ]}
        />
      </SettingsRow>

      <SettingsRow
        label={t('mail_settings_smart_reply', { defaultValue: 'Réponse suggérée' })}
        description={t('mail_settings_smart_reply_desc', { defaultValue: 'Proposer des réponses courtes en un clic.' })}
      >
        <RadioGroup
          value={prefs.smartReply ? 'on' : 'off'}
          onChange={v => set('smartReply', v === 'on')}
          options={[
            { value: 'on',  label: t('mail_settings_enable', { defaultValue: 'Activer' }) },
            { value: 'off', label: t('mail_settings_disable') },
          ]}
        />
      </SettingsRow>

      <SettingsRow
        label={t('mail_settings_nudges', { defaultValue: 'Rappels automatiques' })}
        description={t('mail_settings_nudges_desc', { defaultValue: 'Remonter en haut de la liste les messages à traiter.' })}
      >
        <div className="space-y-2">
          <CheckboxControl
            checked={prefs.nudgesReply}
            onChange={v => set('nudgesReply', v)}
            label={t('mail_settings_nudges_reply', { defaultValue: 'Suggérer les e-mails nécessitant une réponse' })}
          />
          <CheckboxControl
            checked={prefs.nudgesFollowup}
            onChange={v => set('nudgesFollowup', v)}
            label={t('mail_settings_nudges_followup', { defaultValue: "Suggérer les e-mails en attente d'une réponse" })}
          />
        </div>
      </SettingsRow>

      <SettingsRow
        label={t('mail_settings_default_style', { defaultValue: 'Style par défaut du texte' })}
        description={t('mail_settings_default_style_desc', { defaultValue: 'Police, taille et couleur appliquées par défaut au corps des nouveaux messages.' })}
      >
        <DefaultTextStyle
          font={prefs.defaultFont} size={prefs.defaultSize} color={prefs.defaultColor}
          onChange={patch => setPrefs(p => ({
            ...p,
            defaultFont:  patch.font  ?? p.defaultFont,
            defaultSize:  patch.size  ?? p.defaultSize,
            defaultColor: patch.color ?? p.defaultColor,
          }))}
          onReset={() => setPrefs(p => ({
            ...p,
            defaultFont: DEFAULT_TEXT_STYLE.font, defaultSize: DEFAULT_TEXT_STYLE.size, defaultColor: DEFAULT_TEXT_STYLE.color,
          }))}
        />
      </SettingsRow>

      {/* ── Writing help ───────────────────────────────────────────────── */}
      <Section title={t('mail_settings_section_writing', { defaultValue: "Aide à l'écriture" })} />

      <SettingsRow
        label={t('mail_settings_grammar', { defaultValue: 'Grammaire' })}
        description={t('mail_settings_grammar_desc', { defaultValue: 'Souligner les suggestions grammaticales pendant la rédaction.' })}
      >
        <RadioGroup
          value={prefs.grammar ? 'on' : 'off'}
          onChange={v => set('grammar', v === 'on')}
          options={[
            { value: 'on',  label: t('mail_settings_enable', { defaultValue: 'Activer' }) },
            { value: 'off', label: t('mail_settings_disable') },
          ]}
        />
      </SettingsRow>

      <SettingsRow
        label={t('mail_settings_spelling', { defaultValue: 'Orthographe' })}
        description={t('mail_settings_spelling_desc', { defaultValue: 'Souligner les fautes de frappe pendant la rédaction.' })}
      >
        <RadioGroup
          value={prefs.spelling ? 'on' : 'off'}
          onChange={v => set('spelling', v === 'on')}
          options={[
            { value: 'on',  label: t('mail_settings_enable', { defaultValue: 'Activer' }) },
            { value: 'off', label: t('mail_settings_disable') },
          ]}
        />
      </SettingsRow>

      <SettingsRow
        label={t('mail_settings_autocorrect', { defaultValue: 'Correction automatique' })}
        description={t('mail_settings_autocorrect_desc', { defaultValue: 'Corriger automatiquement les fautes courantes.' })}
      >
        <RadioGroup
          value={prefs.autocorrect ? 'on' : 'off'}
          onChange={v => set('autocorrect', v === 'on')}
          options={[
            { value: 'on',  label: t('mail_settings_enable', { defaultValue: 'Activer' }) },
            { value: 'off', label: t('mail_settings_disable') },
          ]}
        />
      </SettingsRow>

      {/* ── Interface ──────────────────────────────────────────────────── */}
      <Section title={t('mail_settings_section_interface', { defaultValue: 'Interface' })} />

      <SettingsRow
        label={t('mail_settings_images')}
        description={t('mail_settings_images_desc')}
      >
        <RadioGroup
          value={prefs.showImages}
          onChange={v => set('showImages', v)}
          options={[
            { value: 'always', label: t('mail_settings_images_always') },
            { value: 'ask',    label: t('mail_settings_images_ask') },
          ]}
        />
      </SettingsRow>

      <SettingsRow
        label={t('mail_settings_hover_actions', { defaultValue: 'Actions de survol' })}
        description={t('mail_settings_hover_actions_desc', { defaultValue: 'Afficher des actions rapides au survol des messages de la liste.' })}
      >
        <RadioGroup
          value={prefs.hoverActions ? 'on' : 'off'}
          onChange={v => set('hoverActions', v === 'on')}
          options={[
            { value: 'on',  label: t('mail_settings_hover_actions_on', { defaultValue: 'Activer les actions de survol' }) },
            { value: 'off', label: t('mail_settings_hover_actions_off', { defaultValue: 'Désactiver les actions de survol' }) },
          ]}
        />
      </SettingsRow>

      <SettingsRow
        label={t('mail_settings_shortcuts', { defaultValue: 'Raccourcis clavier' })}
        description={t('mail_settings_shortcuts_desc', { defaultValue: 'Utiliser les raccourcis clavier pour naviguer et agir vite.' })}
      >
        <RadioGroup
          value={prefs.keyboardShortcuts ? 'on' : 'off'}
          onChange={v => set('keyboardShortcuts', v === 'on')}
          options={[
            { value: 'off', label: t('mail_settings_shortcuts_off', { defaultValue: 'Désactiver les raccourcis clavier' }) },
            { value: 'on',  label: t('mail_settings_shortcuts_on', { defaultValue: 'Activer les raccourcis clavier' }) },
          ]}
        />
      </SettingsRow>

      <SettingsRow
        label={t('mail_settings_button_labels', { defaultValue: 'Libellés des boutons' })}
        description={t('mail_settings_button_labels_desc', { defaultValue: "Afficher les boutons d'action sous forme d'icônes ou de texte." })}
      >
        <RadioGroup
          value={prefs.buttonLabels}
          onChange={v => set('buttonLabels', v)}
          options={[
            { value: 'icons', label: t('mail_settings_button_labels_icons', { defaultValue: 'Icônes' }) },
            { value: 'text',  label: t('mail_settings_button_labels_text', { defaultValue: 'Texte' }) },
          ]}
        />
      </SettingsRow>

      <SettingsRow
        label={t('mail_settings_desktop_notif', { defaultValue: 'Notifications de bureau' })}
        description={t('mail_settings_desktop_notif_desc', { defaultValue: 'Recevoir une notification du navigateur à la réception de nouveaux messages.' })}
      >
        <RadioGroup
          value={prefs.desktopNotifications}
          onChange={v => set('desktopNotifications', v)}
          options={[
            { value: 'all',       label: t('mail_settings_desktop_notif_all', { defaultValue: 'Notifier pour tous les nouveaux messages' }) },
            { value: 'important', label: t('mail_settings_desktop_notif_important', { defaultValue: 'Notifier uniquement pour les messages importants' }) },
            { value: 'off',       label: t('mail_settings_desktop_notif_off', { defaultValue: 'Désactiver les notifications' }) },
          ]}
        />
      </SettingsRow>

      <SettingsRow
        label={t('mail_settings_create_contacts', { defaultValue: 'Créer des contacts pour la saisie semi-automatique' })}
        description={t('mail_settings_create_contacts_desc', { defaultValue: 'Ajouter automatiquement à vos contacts les personnes à qui vous écrivez, pour les proposer ensuite à la saisie.' })}
      >
        <RadioGroup
          value={prefs.createContacts ? 'on' : 'off'}
          onChange={v => set('createContacts', v === 'on')}
          options={[
            { value: 'on',  label: t('mail_settings_create_contacts_on', { defaultValue: "Ajouter les contacts à mesure que j'écris" }) },
            { value: 'off', label: t('mail_settings_create_contacts_off', { defaultValue: 'Ajouter les contacts manuellement' }) },
          ]}
        />
      </SettingsRow>

      {/* ── Vacation responder (auto-reply) ────────────────────────────── */}
      <Section title={t('mail_settings_section_vacation', { defaultValue: 'Réponse automatique' })} />

      {vacError && (
        <Callout variant="danger" className="my-4">
          {vacError}
        </Callout>
      )}

      <SettingsRow label={t('mail_settings_vacation_status', { defaultValue: 'Réponse automatique' })}>
        <RadioGroup
          value={prefs.vacation.enabled ? 'on' : 'off'}
          onChange={v => setVac('enabled', v === 'on')}
          options={[
            { value: 'off', label: t('mail_settings_vacation_off', { defaultValue: 'Réponse automatique désactivée' }) },
            { value: 'on',  label: t('mail_settings_vacation_on', { defaultValue: 'Réponse automatique activée' }) },
          ]}
        />
      </SettingsRow>

      {prefs.vacation.enabled && (
        <>
          <SettingsRow
            label={t('mail_settings_vacation_dates', { defaultValue: 'Période' })}
            description={t('mail_settings_vacation_dates_desc', { defaultValue: 'Premier jour (obligatoire) et dernier jour (facultatif) de la réponse automatique.' })}
          >
            <div className="flex flex-wrap items-end gap-4">
              <label className="block">
                <span className="block text-xs text-text-secondary mb-1">{t('mail_settings_vacation_start', { defaultValue: 'Premier jour' })}</span>
                <input
                  type="date"
                  value={prefs.vacation.startDate}
                  onChange={e => setVac('startDate', e.target.value)}
                  className="h-9 px-3 text-sm rounded-lg border border-border bg-surface-0 focus:outline-none focus:border-primary"
                />
              </label>
              <label className="block">
                <span className="block text-xs text-text-secondary mb-1">{t('mail_settings_vacation_end', { defaultValue: 'Dernier jour (facultatif)' })}</span>
                <input
                  type="date"
                  value={prefs.vacation.endDate}
                  onChange={e => setVac('endDate', e.target.value)}
                  className="h-9 px-3 text-sm rounded-lg border border-border bg-surface-0 focus:outline-none focus:border-primary"
                />
              </label>
            </div>
          </SettingsRow>

          <SettingsRow label={t('mail_settings_vacation_subject', { defaultValue: 'Objet' })}>
            <Input
              value={prefs.vacation.subject}
              onChange={e => setVac('subject', e.target.value)}
              placeholder={t('mail_settings_vacation_subject_ph', { defaultValue: 'Absent(e) du bureau' })}
              className="max-w-md"
            />
          </SettingsRow>

          <SettingsRow label={t('mail_settings_vacation_message', { defaultValue: 'Message' })}>
            <RichTextEditor
              value={prefs.vacation.messageHtml}
              onChange={html => setVac('messageHtml', html)}
              placeholder={t('mail_settings_vacation_message_ph', { defaultValue: 'Je suis actuellement absent(e) et vous répondrai à mon retour…' })}
              minHeight={140}
            />
          </SettingsRow>

          <SettingsRow label={t('mail_settings_vacation_scope', { defaultValue: 'Destinataires' })}>
            <CheckboxControl
              checked={prefs.vacation.contactsOnly}
              onChange={v => setVac('contactsOnly', v)}
              label={t('mail_settings_vacation_contacts_only', { defaultValue: 'Répondre uniquement aux personnes de mes contacts' })}
            />
          </SettingsRow>
        </>
      )}

      {/* ── Signature ──────────────────────────────────────────────────── */}
      <Section id="signatures" title={t('mail_settings_section_signature', { defaultValue: 'Signature' })} />

      <div className="py-4 border-b border-[#e8eaed]">
        <div className="flex items-center justify-between mb-4">
          <p className="text-sm text-text-tertiary">
            {t('mail_sig_intro', { defaultValue: 'Vos signatures, insérables depuis le composeur (icône stylo).' })}
          </p>
          <button onClick={addSig} className="flex items-center gap-1.5 text-sm text-primary hover:underline">
            <Plus size={16} /> {t('mail_sig_add', { defaultValue: 'Ajouter une signature' })}
          </button>
        </div>

        {sigList.length === 0 ? (
          <p className="text-sm text-text-tertiary py-6 text-center">
            {t('mail_sig_empty', { defaultValue: 'Aucune signature. Ajoutez-en une.' })}
          </p>
        ) : (
          <ul className="space-y-5">
            {sigList.map(s => (
              <li key={s.id} className="border border-border rounded-xl p-4">
                <div className="flex items-center gap-3 mb-2">
                  <Input value={s.name} onChange={e => updateSig(s.id, { name: e.target.value })}
                    placeholder={t('mail_sig_name', { defaultValue: 'Nom de la signature' })} className="flex-1" />
                  <button
                    onClick={() => setDefSig(d => (d === s.id ? '' : s.id))}
                    className={`flex items-center gap-1 h-8 px-3 rounded-full text-sm border transition-colors ${
                      defSig === s.id ? 'bg-primary/10 border-primary/40 text-primary' : 'border-border text-text-secondary hover:bg-surface-1'
                    }`}
                    title={t('mail_sig_default', { defaultValue: 'Signature par défaut' })}
                  >
                    {defSig === s.id && <Check size={14} />}
                    {t('mail_sig_default', { defaultValue: 'Par défaut' })}
                  </button>
                  <button onClick={() => removeSig(s.id)}
                    className="p-1.5 rounded text-text-tertiary hover:text-danger hover:bg-danger/10"
                    title={t('delete', { defaultValue: 'Supprimer' })}>
                    <Trash2 size={16} />
                  </button>
                </div>
                <RichTextEditor
                  value={s.html}
                  onChange={html => updateSig(s.id, { html })}
                  placeholder={t('mail_sig_html_ph', { defaultValue: 'Contenu — ex. votre nom, fonction, coordonnées…' })}
                  minHeight={110}
                />
              </li>
            ))}
          </ul>
        )}
      </div>

      <SettingsRow
        label={t('mail_settings_sig_defaults', { defaultValue: 'Valeurs par défaut de la signature' })}
        description={t('mail_settings_sig_defaults_desc', { defaultValue: 'Quelle signature préremplir selon le type de message.' })}
      >
        <div className="space-y-4 max-w-sm">
          <label className="block">
            <span className="block text-sm text-text-secondary mb-1">
              {t('mail_settings_sig_new', { defaultValue: 'Dans les nouveaux e-mails' })}
            </span>
            <Dropdown
              value={prefs.signatureNew}
              onChange={v => set('signatureNew', v)}
              options={sigOptions}
              width="100%" height={36} fontSize={14} focusable
            />
          </label>
          <label className="block">
            <span className="block text-sm text-text-secondary mb-1">
              {t('mail_settings_sig_reply', { defaultValue: 'Dans les réponses / transferts' })}
            </span>
            <Dropdown
              value={prefs.signatureReply}
              onChange={v => set('signatureReply', v)}
              options={sigOptions}
              width="100%" height={36} fontSize={14} focusable
            />
          </label>
        </div>
      </SettingsRow>

      {senderAddresses.length > 0 && (
        <SettingsRow
          label={t('mail_settings_sig_per_address', { defaultValue: 'Signatures par défaut par adresse' })}
          description={t('mail_settings_sig_per_address_desc', { defaultValue: "Choisir, pour chaque adresse d'expédition, la signature préremplie selon le type de message. « Valeur par défaut » applique le réglage global ci-dessus." })}
        >
          <div className="space-y-5 max-w-2xl">
            {senderAddresses.map(addr => (
              <div key={addr.email} className="border border-border rounded-xl p-4">
                <div className="text-sm font-medium text-text-primary mb-3 truncate">{addr.label}</div>
                <div className="grid gap-4 sm:grid-cols-2">
                  <label className="block">
                    <span className="block text-sm text-text-secondary mb-1">
                      {t('mail_settings_sig_new', { defaultValue: 'Dans les nouveaux e-mails' })}
                    </span>
                    <Dropdown
                      value={perAddrValue(addr.email, 'new')}
                      onChange={v => setPerAddr(addr.email, 'new', v)}
                      options={perAddrOptions}
                      width="100%" height={36} fontSize={14} focusable
                    />
                  </label>
                  <label className="block">
                    <span className="block text-sm text-text-secondary mb-1">
                      {t('mail_settings_sig_reply', { defaultValue: 'Dans les réponses / transferts' })}
                    </span>
                    <Dropdown
                      value={perAddrValue(addr.email, 'reply')}
                      onChange={v => setPerAddr(addr.email, 'reply', v)}
                      options={perAddrOptions}
                      width="100%" height={36} fontSize={14} focusable
                    />
                  </label>
                </div>
              </div>
            ))}
          </div>
        </SettingsRow>
      )}

      {/* ── Save ───────────────────────────────────────────────────────── */}
      <div className="pt-5 flex items-center gap-3">
        <Button onClick={save}>
          {saved
            ? <><Check size={14} className="mr-1.5 inline" />{t('mail_settings_saved')}</>
            : t('mail_settings_save_changes')
          }
        </Button>
        <Button variant="ghost" onClick={() => { setPrefs(DEFAULT_PREFS) }}>
          {t('common_cancel')}
        </Button>
      </div>
    </div>
  )
}
