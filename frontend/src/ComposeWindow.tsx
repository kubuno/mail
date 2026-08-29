import { useState, useRef, useEffect, useCallback } from 'react'
import { useTranslation } from 'react-i18next'
import { openImagePicker } from '@kubuno/sdk'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { FloatingWindow, Dropdown, MenuDropdown, useIsMobile, serializeMentions, type MenuItem, type MenuDropdownPos } from '@ui'
import { prompt } from '@kubuno/sdk'
import { useUndoSendStore } from './undoSendStore'
import {
  X, Minus, Square, Copy,
  AlignLeft, AlignCenter, AlignRight,
  Eraser, Check, Tag, Star, FileText,
} from 'lucide-react'
import { mailApi, EmailAddress, apiErrorMessage } from './api'
import { activeSenders, defaultComposeSender } from './senderSelection'
import { RecipientField } from './AddressSuggest'
import { useMailStore } from './store'
import { readKubunoData, kubunoDataToEmailHtml } from './kubunoData'
import { useDraftAutosave, type DraftSnapshot } from './mail-app/useDraftAutosave'
import { loadSignatures, defaultSignatureId, resolveSignatureId } from './signatures'
import { loadPrefs } from './settings/GeneralTab'
import { signatureBlock, shouldShowPlaceholder, placeCaretAtStart, appendDriveLinksHtml, SIGNATURE_ATTR } from './mail-app/composeContent'
import { useComposeAttachments } from './mail-app/composeAttachments'
import { useComposeMentions } from './mail-app/useComposeMentions'
import AttachmentBar from './mail-app/AttachmentChip'
import LargeFilesModal from './mail-app/LargeFilesModal'
import { useMaxMessageSize } from './mail-app/useMaxMessageSize'
import { LabelChecklist } from './mail-app/compose/parts'
import ComposeFormatToolbar from './mail-app/compose/ComposeFormatToolbar'
import ComposeActionBar from './mail-app/compose/ComposeActionBar'

const MIN_W = 420, MIN_H = 320

export default function ComposeWindow() {
  const { t } = useTranslation('mail')
  const isMobile = useIsMobile()
  const { setComposeOpen, accounts, composeInitial, setComposeInitial } = useMailStore()
  const scheduleUndo = useUndoSendStore(s => s.schedule)
  const sendFailed   = useUndoSendStore(s => s.failed)
  const qc = useQueryClient()

  // Only active accounts can send: a deactivated one (its mailbox was deleted)
  // must not appear as a possible sender.
  const activeAccounts = activeSenders(accounts)
  const defaultAccount = defaultComposeSender(accounts)
  // Sender identity: several accounts can be configured, the user picks one
  // in the "From" row (defaults to the default account).
  const [fromId, setFromId] = useState(defaultAccount?.id ?? '')
  useEffect(() => {
    if (!fromId && defaultAccount) setFromId(defaultAccount.id)
  }, [fromId, defaultAccount])
  const fromAccount = activeAccounts.find(a => a.id === fromId) ?? defaultAccount
  const accountId = fromAccount?.id ?? ''

  const [to,        setTo]        = useState<EmailAddress[]>(composeInitial?.to ?? [])
  const [cc,        setCc]        = useState<EmailAddress[]>(composeInitial?.cc ?? [])
  const [showCc,    setShowCc]    = useState(!!composeInitial?.cc.length)
  const [bcc,       setBcc]       = useState<EmailAddress[]>(composeInitial?.bcc ?? [])
  const [showBcc,   setShowBcc]   = useState(!!composeInitial?.bcc?.length)
  const [subject,   setSubject]   = useState(composeInitial?.subject ?? '')
  const [showFmt,   setShowFmt]   = useState(true)
  // Plain-text mode (Gmail "Texte brut") strips the current body's markup and
  // hides the formatting affordances. Spellcheck backs the body's spellCheck attr.
  const [plainText, setPlainText] = useState(false)
  const [spellcheck, setSpellcheck] = useState(true)
  const [minimized, setMinimized] = useState(false)
  // Full-screen composer (Gmail "Plein écran"). The default is remembered in
  // localStorage; opening a composer honours it. Toggling the default also
  // applies it to the current composer (matching Gmail).
  const readFullscreenDefault = () => {
    try { return localStorage.getItem('mail-compose-fullscreen-default') === '1' } catch { return false }
  }
  const [fullscreenDefault, setFullscreenDefault] = useState(readFullscreenDefault)
  // The « Fenêtre séparée » pop-out opens a maximized composer regardless of the
  // stored default.
  const [maximized, setMaximized] = useState(() => readFullscreenDefault() || !!composeInitial?.fullscreen)
  const [fontName,  setFontName]  = useState('Arial')
  const [fontSz,    setFontSz]    = useState('13')

  const bodyRef = useRef<HTMLDivElement>(null)
  // Thread being replied to, captured once: `composeInitial` is cleared right
  // after the init effect runs, so we keep the value for the draft snapshot and
  // the send payload (resuming a reply draft must still thread correctly).
  const replyToIdRef = useRef<string | undefined>(composeInitial?.replyToId)
  // « Message programmé »: captured at first render because `composeInitial` is
  // cleared right after the init effect — the mount effect below reads the ref.
  const scheduleOnMountRef = useRef(!!composeInitial?.schedule)

  // Gmail-style placeholder: shown while the body is "logically empty" — i.e. it
  // holds nothing but the auto-inserted signature. It hides as soon as the user
  // types real content above it. Recomputed on mount and on every input.
  const [showPlaceholder, setShowPlaceholder] = useState(true)
  const refreshPlaceholder = useCallback(() => {
    setShowPlaceholder(shouldShowPlaceholder(bodyRef.current))
  }, [])

  // ── Auto-save as a draft (Gmail-style) ───────────────────────────────────────
  const getSnapshot = useCallback((): DraftSnapshot | null => {
    if (!accountId) return null
    return {
      account_id:    accountId,
      to_addresses:  to,
      cc_addresses:  cc,
      bcc_addresses: bcc,
      subject,
      body_html:     bodyRef.current?.innerHTML ?? '',
      reply_to_id:   replyToIdRef.current,
    }
  }, [accountId, to, cc, bcc, subject])
  // Reopening after "undo send" carries the draft id so we keep updating the
  // SAME draft rather than spawning a duplicate.
  const draft = useDraftAutosave(getSnapshot, composeInitial?.draftId)
  // A field change (recipients, subject) schedules a save.
  useEffect(() => { draft.touch() }, [to, cc, bcc, subject]) // eslint-disable-line react-hooks/exhaustive-deps

  // @mention in the body: dropdown of contacts, chip insertion, mailto on send.
  // A body mutation (typing OR a chip in/out) touches autosave + the placeholder,
  // exactly like a plain input.
  const mentions = useComposeMentions(bodyRef, useCallback(() => {
    draft.touch(); refreshPlaceholder()
  }, [draft, refreshPlaceholder]))

  // Pré-remplissage (ré-ouverture après « Annuler l'envoi ») + signature par défaut.
  useEffect(() => {
    const html = composeInitial?.bodyHtml ?? ''
    requestAnimationFrame(() => {
      const el = bodyRef.current
      if (el && html) el.innerHTML = html
      // Auto-insert the default "new message" signature (Gmail-style), UNLESS the
      // body was prefilled (resuming a draft — it already carries its content) or
      // already contains a signature block. The caret sits ABOVE it, so the user
      // writes on top of the signature.
      if (el && !html && !el.querySelector(`[${SIGNATURE_ATTR}]`)) {
        // Resolve in the users' expected order: the active sender's per-address
        // default → the global « new mail » preference → the single default sig.
        const sigId = resolveSignatureId(fromAccount?.email_address, 'new', loadPrefs().signatureNew)
        const sig = loadSignatures().find(s => s.id === sigId)
        if (sig) {
          el.innerHTML = signatureBlock(sig.html)
          placeCaretAtStart(el)
        }
      }
      refreshPlaceholder()
      // Baseline AFTER the prefill + signature, so "changed since open" is
      // measured against what the user was handed, not against empty.
      draft.captureBaseline()
    })
    if (composeInitial) setComposeInitial(null)
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // ── Signature par adresse d'expédition ────────────────────────────────────────
  // When the user changes the « De » address, replace ONLY the marked signature
  // block by the one that address defaults to — the user's own text is untouched
  // (Gmail parity). Skips the very first run (the init effect owns the initial sig).
  const prevFromEmailRef = useRef<string | null>(null)
  useEffect(() => {
    const email = fromAccount?.email_address ?? ''
    if (prevFromEmailRef.current === null) { prevFromEmailRef.current = email; return }
    if (prevFromEmailRef.current === email) return
    prevFromEmailRef.current = email
    const el = bodyRef.current
    if (!el) return
    const sigId = resolveSignatureId(email, 'new', loadPrefs().signatureNew)
    const html = loadSignatures().find(s => s.id === sigId)?.html
    const existing = el.querySelector(`[${SIGNATURE_ATTR}]`)
    if (html) {
      const tpl = document.createElement('template')
      tpl.innerHTML = signatureBlock(html)
      const block = tpl.content.firstChild
      if (existing && block) existing.replaceWith(block)
      else if (block) el.appendChild(block)
    } else if (existing) {
      existing.remove()
    }
    draft.touch()
    refreshPlaceholder()
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fromAccount?.email_address])

  // Envoi immédiat → différé de 5 s avec possibilité d'annuler (toast dans MailApp).
  const sendNow = () => {
    if (!to.length || !accountId) return
    // Serialize the editor's mention chips to `mailto:` links before anything is
    // appended (confidential note, Drive links carry no chips).
    let body = serializeMentions(bodyRef.current?.innerHTML ?? '', 'mailto')
    if (confidential) {
      body += `<br><br><div style="border-top:1px solid #dadce0;color:#5f6368;font-size:12px;padding-top:6px">🔒 ${t('mail_confidential_note', { defaultValue: 'Ce message est confidentiel.' })}</div>`
    }
    // Oversized attachments travel as public Drive links appended to the body.
    body = appendDriveLinksHtml(body, atts.driveLinks(), driveLinksIntro)
    const payload = {
      account_id:   accountId,
      to_addresses: to,
      cc_addresses: cc.length ? cc : undefined,
      bcc_addresses: bcc.length ? bcc : undefined,
      subject,
      body_html:    body,
      reply_to_id:  replyToIdRef.current,
      attachments:  atts.payload().length ? atts.payload() : undefined,
      sign:         pgpSign || undefined,
      encrypt:      pgpEncrypt || undefined,
      label_ids:    labelSel.size ? [...labelSel] : undefined,
    }
    // Stop auto-saving; the actual send (5s later, after the undo window) reads
    // the draft id then — so any create still in flight has landed — and hands
    // it over for the backend to delete once the message goes out.
    draft.markSent()
    scheduleUndo(payload, () => {
      mailApi.sendMail({ ...payload, draft_id: draft.draftId() ?? undefined })
        .then(() => { qc.invalidateQueries({ queryKey: ['mail-threads'] }); qc.invalidateQueries({ queryKey: ['mail-counts'] }); qc.invalidateQueries({ queryKey: ['mail-drafts'] }) })
        .catch(err => sendFailed(apiErrorMessage(err, t('mail_send_failed', { defaultValue: "L'envoi a échoué." }))))
    })
    setComposeOpen(false)
  }

  // ── execCommand ─────────────────────────────────────────────────────────────
  // The font/size selectors are editable inputs that steal focus from the body,
  // collapsing its selection. We track the last body selection and restore it
  // before applying a command so formatting lands on the intended text.
  const savedRangeRef = useRef<Range | null>(null)
  useEffect(() => {
    const save = () => {
      const sel = document.getSelection()
      if (sel && sel.rangeCount && bodyRef.current?.contains(sel.getRangeAt(0).commonAncestorContainer)) {
        savedRangeRef.current = sel.getRangeAt(0).cloneRange()
      }
    }
    document.addEventListener('selectionchange', save)
    return () => document.removeEventListener('selectionchange', save)
  }, [])
  const restoreSel = () => {
    bodyRef.current?.focus()
    const r = savedRangeRef.current
    const sel = document.getSelection()
    if (r && sel) { sel.removeAllRanges(); sel.addRange(r) }
  }

  const exec = (cmd: string, value?: string) => {
    restoreSel()
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    ;(document as any).execCommand(cmd, false, value ?? undefined)
  }

  // Apply an arbitrary pixel font-size to the selection. execCommand('fontSize')
  // only accepts the legacy 1-7 scale, so we tag the selection with size 7 then
  // rewrite those <font> markers to the real px value (the classic reliable trick).
  const applyFontSizePx = (px: string) => {
    const ed = bodyRef.current
    if (!ed) return
    restoreSel()
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const cmd = (c: string, v?: string) => (document as any).execCommand(c, false, v)
    cmd('styleWithCSS', 'false')
    cmd('fontSize', '7')
    ed.querySelectorAll('font[size="7"]').forEach(el => {
      const e = el as HTMLElement
      e.removeAttribute('size')
      e.style.fontSize = `${px}px`
    })
  }

  // Print the composed body. We deliberately avoid window.open('', '_blank'):
  // that leaves a child window attached to the page whose focus/execCommand
  // state silently disables editing everywhere (body, subject, recipients) —
  // and even freshly opened composers — until a full page reload. Instead we
  // print through an off-screen iframe that we fully own and remove afterwards,
  // leaving the main document's focus and selection untouched.
  const printBody = () => {
    const html = bodyRef.current?.innerHTML ?? ''
    const iframe = document.createElement('iframe')
    iframe.setAttribute('aria-hidden', 'true')
    Object.assign(iframe.style, {
      position: 'fixed', right: '0', bottom: '0',
      width: '0', height: '0', border: '0', visibility: 'hidden',
    })
    document.body.appendChild(iframe)

    const win = iframe.contentWindow
    const doc = iframe.contentDocument
    if (!win || !doc) { iframe.remove(); bodyRef.current?.focus(); return }

    // Guarded teardown: runs at most once, removes the iframe and returns focus
    // to the composer so the user can keep typing right away.
    let done = false
    const cleanup = () => {
      if (done) return
      done = true
      win.removeEventListener('afterprint', cleanup)
      iframe.remove()
      bodyRef.current?.focus()
    }
    win.addEventListener('afterprint', cleanup)

    doc.open()
    doc.write(`<!doctype html><html><head><meta charset="utf-8"></head><body>${html}</body></html>`)
    doc.close()

    // Let the iframe lay out, then print. window.print() is synchronous in the
    // browsers we target (afterprint fires before it returns), so the trailing
    // cleanup() is a guarded no-op there; it also guarantees teardown when
    // afterprint never fires (e.g. a stubbed print in tests).
    setTimeout(() => {
      try { win.focus(); win.print() } catch { /* ignore print failures */ }
      cleanup()
    }, 50)
  }

  // ── Pièces jointes ────────────────────────────────────────────────────────────
  // Enriched state (progress / cancel / preview) lives in a shared hook; here the
  // file input only feeds it. Attachments are base64-encoded locally and travel
  // in the /send payload (no upload endpoint yet).
  const fileRef = useRef<HTMLInputElement>(null)
  // Files over the admin max message size are uploaded to Drive as public links
  // instead of being base64-encoded into /send (Gmail parity).
  const { maxMb, maxBytes } = useMaxMessageSize()
  const atts = useComposeAttachments(composeInitial?.attachments, { maxBytes })
  const driveLinksIntro = t('mail_drive_links_intro', { defaultValue: 'Fichiers partagés via Drive :' })
  const onPickFiles = (files: FileList | null) => {
    atts.addFiles(files)
    if (fileRef.current) fileRef.current.value = ''
  }

  // ── Drag & drop of files onto the composer (Gmail-style « Déposez ici ») ──────
  // Only a file drag lights up the drop zone; dragging selected text inside the
  // editor carries no `Files` type and is ignored. dragenter/dragleave fire on
  // every child, so a depth counter keeps the overlay stable until the pointer
  // truly leaves the composer.
  const [dragOver, setDragOver] = useState(false)
  const dragDepth = useRef(0)
  const isFileDrag = (e: React.DragEvent) => e.dataTransfer?.types?.includes('Files')
  const onDragEnter = (e: React.DragEvent) => {
    if (!isFileDrag(e)) return
    e.preventDefault()
    dragDepth.current += 1
    setDragOver(true)
  }
  const onDragOver = (e: React.DragEvent) => {
    if (!isFileDrag(e)) return
    e.preventDefault()
    e.dataTransfer.dropEffect = 'copy'
  }
  const onDragLeave = (e: React.DragEvent) => {
    if (!isFileDrag(e)) return
    dragDepth.current = Math.max(0, dragDepth.current - 1)
    if (dragDepth.current === 0) setDragOver(false)
  }
  const onDropFiles = (e: React.DragEvent) => {
    if (!isFileDrag(e)) return
    // Prevent the browser (and the contenteditable) from inserting the file.
    e.preventDefault()
    dragDepth.current = 0
    setDragOver(false)
    if (e.dataTransfer.files?.length) atts.addFiles(e.dataTransfer.files)
  }

  // ── Boutons de la barre d'action ───────────────────────────────────────────────
  const insertLink = async () => {
    const url = await prompt({ title: t('mail_insert_link', { defaultValue: 'Insérer un lien' }), placeholder: 'https://…' })
    if (url?.trim()) exec('createLink', url.trim())
  }
  const insertImageUrl = async () => {
    // A recipient reads the message elsewhere, so an uploaded file would have
    // to be hosted first: only addressable sources make sense here.
    const picked = await openImagePicker({
      title: t('mail_insert_image', { defaultValue: 'Insérer une image' }),
      exclude: ['upload', 'webcam'],
    })
    if (picked?.kind === 'url') exec('insertImage', picked.url)
  }
  const [alignMenu,    setAlignMenu]    = useState<MenuDropdownPos | null>(null)
  const [confidential, setConfidential] = useState(false)
  // OpenPGP: sign / encrypt this message. The control only appears when the
  // instance admin enabled the feature.
  // Pre-armed when opened from « Message chiffré (PGP) ».
  const [pgpSign,    setPgpSign]    = useState(() => !!composeInitial?.secure)
  const [pgpEncrypt, setPgpEncrypt] = useState(() => !!composeInitial?.secure)
  const { data: pgpStatus } = useQuery({ queryKey: ['mail-pgp-status'], queryFn: mailApi.pgpStatus })
  const gpgEnabled = !!pgpStatus?.enabled
  const secItems: MenuItem[] = [
    { type: 'action', label: t('mail_pgp_sign', { defaultValue: 'Signer numériquement' }),
      icon: pgpSign ? <Check size={15} /> : <span className="inline-block w-[15px]" />, onClick: () => setPgpSign(v => !v) },
    { type: 'action', label: t('mail_pgp_encrypt', { defaultValue: 'Chiffrer (OpenPGP)' }),
      icon: pgpEncrypt ? <Check size={15} /> : <span className="inline-block w-[15px]" />, onClick: () => setPgpEncrypt(v => !v) },
  ]

  // Signatures menu (pen icon), Gmail-style: manage + none + each named signature.
  const signatures = loadSignatures()
  const defaultSigId = defaultSignatureId()
  const insertSig = (html: string) => exec('insertHTML', `<br><br>${html}`)
  const sigItems: MenuItem[] = [
    { type: 'action', label: t('mail_sig_manage', { defaultValue: 'Gérer les signatures' }),
      onClick: () => { window.location.href = '/mail/settings#signatures' } },
    { type: 'separator' },
    { type: 'action', label: t('mail_sig_none', { defaultValue: 'Aucune signature' }),
      icon: <span className="inline-block w-[15px]" />, onClick: () => { /* insert nothing */ } },
    ...signatures.map<MenuItem>(s => ({
      type: 'action', label: s.name,
      icon: s.id === defaultSigId ? <Check size={15} /> : <span className="inline-block w-[15px]" />,
      onClick: () => insertSig(s.html),
    })),
  ]
  // Enabling plain-text mode flattens the current body to its text content;
  // disabling it simply re-allows markup (the text stays as-is).
  const togglePlainText = () => {
    setPlainText(prev => {
      const next = !prev
      if (next && bodyRef.current) {
        const text = bodyRef.current.innerText
        bodyRef.current.textContent = text
        draft.touch()
        refreshPlaceholder()
      }
      return next
    })
  }
  // Labels selectable from the « ⋯ » → Libellé submenu (like Gmail). Selection is
  // held here; applying labels to the SENT message is a backend follow-up (the
  // send call doesn't take label_ids yet).
  const { data: labelData } = useQuery({ queryKey: ['mail-labels'], queryFn: () => mailApi.listLabels() })
  const composeLabels = labelData?.labels ?? []
  const [labelSel, setLabelSel] = useState<Set<string>>(new Set())
  const [follow, setFollow] = useState(false)
  const toggleLabel = (id: string) => setLabelSel(s => { const n = new Set(s); n.has(id) ? n.delete(id) : n.add(id); return n })

  // Toggle the "full screen by default" preference. Persist it and, like Gmail,
  // apply it to the composer that is currently open.
  const toggleFullscreenDefault = () => {
    setFullscreenDefault(prev => {
      const next = !prev
      try { localStorage.setItem('mail-compose-fullscreen-default', next ? '1' : '0') } catch { /* private mode: ignore */ }
      setMaximized(next)
      return next
    })
  }

  // « Enregistrer comme modèle » — capture the current subject + body as a
  // reusable template (the backend enforces a unique, non-empty name).
  const saveAsTemplate = async () => {
    const name = await prompt({
      title:       t('mail_save_as_template', { defaultValue: 'Enregistrer comme modèle' }),
      message:     t('mail_template_name_prompt', { defaultValue: 'Nom du modèle :' }),
      placeholder: t('mail_template_name_ph', { defaultValue: 'Ex. Relance client' }),
    })
    if (!name?.trim()) return
    try {
      await mailApi.createTemplate({ name: name.trim(), subject, bodyHtml: bodyRef.current?.innerHTML ?? '' })
      qc.invalidateQueries({ queryKey: ['mail-templates'] })
    } catch (e) {
      // Surface the backend message (empty name / duplicate) via the shared toast path.
      sendFailed(apiErrorMessage(e, t('mail_template_save_failed', { defaultValue: "Le modèle n'a pas pu être enregistré." })))
    }
  }

  const moreItems: MenuItem[] = [
    { type: 'action', label: t('mail_fullscreen_default', { defaultValue: 'Plein écran par défaut' }),
      icon: fullscreenDefault ? <Check size={15} /> : <span className="inline-block w-[15px]" />, onClick: toggleFullscreenDefault },
    { type: 'separator' },
    { type: 'action', label: t('mail_save_as_template', { defaultValue: 'Enregistrer comme modèle' }),
      icon: <FileText size={15} />, onClick: saveAsTemplate },
    { type: 'separator' },
    { type: 'action', label: t('mail_plain_text', { defaultValue: 'Mode Texte brut' }),
      icon: plainText ? <Check size={15} /> : <span className="inline-block w-[15px]" />, onClick: togglePlainText },
    { type: 'separator' },
    { type: 'action', label: t('mail_print', { defaultValue: 'Imprimer' }), icon: <span className="inline-block w-[15px]" />, onClick: printBody },
    { type: 'action', label: t('mail_spellcheck', { defaultValue: 'Correcteur orthographique' }),
      icon: spellcheck ? <Check size={15} /> : <span className="inline-block w-[15px]" />, onClick: () => setSpellcheck(v => !v) },
    { type: 'action', label: t('mail_clear_format', { defaultValue: 'Effacer la mise en forme' }), icon: <Eraser size={15} />, onClick: () => exec('removeFormat') },
    { type: 'separator' },
    { type: 'submenu', icon: <Tag size={15} />, label: t('mail_add_label', { defaultValue: 'Libellé' }), items: [
      { type: 'custom', render: () => <LabelChecklist labels={composeLabels} checked={labelSel} onToggle={toggleLabel} placeholder={t('mail_assign_label', { defaultValue: 'Attribuer le libellé :' })} /> },
      { type: 'separator' },
      { type: 'action', icon: <Star size={15} className={follow ? 'text-yellow-500' : ''} />, label: t('mail_enable_follow', { defaultValue: 'Activer le suivi' }), onClick: () => setFollow(v => !v) },
      { type: 'separator' },
      { type: 'action', label: t('common_create', { defaultValue: 'Créer' }), onClick: () => { window.location.href = '/mail/settings' } },
      { type: 'action', label: t('mail_manage_labels', { defaultValue: 'Gérer les libellés' }), onClick: () => { window.location.href = '/mail/settings' } },
    ] },
  ]
  // The three alignment buttons collapse into one dropdown to keep the toolbar tidy.
  const alignItems: MenuItem[] = [
    { type: 'action', label: t('mail_align_left'),   icon: <AlignLeft size={15} />,   onClick: () => exec('justifyLeft') },
    { type: 'action', label: t('mail_align_center'), icon: <AlignCenter size={15} />, onClick: () => exec('justifyCenter') },
    { type: 'action', label: t('mail_align_right'),  icon: <AlignRight size={15} />,  onClick: () => exec('justifyRight') },
  ]

  // ── Send ────────────────────────────────────────────────────────────────────
  const sendMut = useMutation({
    mutationFn: async (scheduledAt?: string) => {
      // Ensure the draft exists so the scheduled send can supersede it, then
      // stop auto-saving.
      const draftId = await draft.flush()
      draft.markSent()
      return mailApi.sendMail({
        account_id:   accountId,
        to_addresses: to,
        cc_addresses: cc.length ? cc : undefined,
        bcc_addresses: bcc.length ? bcc : undefined,
        subject,
        body_html:    appendDriveLinksHtml(serializeMentions(bodyRef.current?.innerHTML ?? '', 'mailto'), atts.driveLinks(), driveLinksIntro),
        reply_to_id:  replyToIdRef.current,
        sign:         pgpSign || undefined,
        encrypt:      pgpEncrypt || undefined,
        scheduled_at: scheduledAt,
        draft_id:     draftId ?? undefined,
        label_ids:    labelSel.size ? [...labelSel] : undefined,
      })
    },
    onSuccess: () => {
      setComposeOpen(false)
      qc.invalidateQueries({ queryKey: ['mail-threads'] })
      qc.invalidateQueries({ queryKey: ['mail-scheduled'] })
      qc.invalidateQueries({ queryKey: ['mail-counts'] })
    },
  })

  // ── Minimized bar ───────────────────────────────────────────────────────────
  // On mobile the window is full screen (FloatingWindow fullBleed): minimizing
  // makes no sense, and a leftover minimized state (desktop → rotate) reopens.
  if (minimized && !isMobile) {
    return (
      <div
        className="fixed bottom-0 right-4 w-72 bg-[#404040] rounded-t-xl shadow-xl z-50 flex items-center justify-between px-4 py-2.5 cursor-pointer"
        onClick={() => setMinimized(false)}
      >
        <span className="text-sm text-white font-medium truncate">{subject || t('new_message')}</span>
        <div className="flex items-center gap-2">
          <button onMouseDown={e => e.stopPropagation()} onClick={e => { e.stopPropagation(); setMinimized(false) }} className="text-white/70 hover:text-white"><Minus size={13} /></button>
          <button onMouseDown={e => e.stopPropagation()} onClick={e => { e.stopPropagation(); setComposeOpen(false) }} className="text-white/70 hover:text-white"><X size={13} /></button>
        </div>
      </div>
    )
  }

  // Composer content, rendered IDENTICALLY in windowed and full-screen modes.
  const composerBody = (
    <div
      className="relative flex flex-col flex-1 min-h-0"
      onDragEnter={onDragEnter}
      onDragOver={onDragOver}
      onDragLeave={onDragLeave}
      onDrop={onDropFiles}
    >
      {/* ── Zone de dépôt (glisser-déposer de fichiers) ───────────────────────── */}
      {dragOver && (
        <div className="pointer-events-none absolute inset-2 z-30 flex items-center justify-center rounded-xl border-2 border-dashed border-primary bg-white/70 backdrop-blur-[2px]">
          <span className="text-2xl font-medium text-text-tertiary select-none">
            {t('mail_drop_files', { defaultValue: 'Déposez les fichiers ici' })}
          </span>
        </div>
      )}
      {/* ── Expéditeur (choix du compte quand plusieurs sont configurés) ───────── */}
      {activeAccounts.length > 1 && (
        <div className="flex items-center gap-3 px-4 py-1.5 border-b border-border flex-shrink-0">
          <span className="text-[14px] text-text-tertiary flex-shrink-0">{t('mail_filter_from')}</span>
          <Dropdown
            value={accountId}
            onChange={setFromId}
            options={activeAccounts.map(a => ({ value: a.id, label: `${a.name} <${a.email_address}>` }))}
            variant="ghost"
            height={28}
            fontSize={14}
          />
        </div>
      )}

      {/* ── Destinataires (autocomplétion : index mail + contacts) ─────────────── */}
      {/* RecipientField is shared (AddressSuggest.tsx); bump its input + chips to
          14px locally via scoped arbitrary variants so InlineCompose is untouched. */}
      <div className="flex items-start gap-2 px-4 py-2.5 border-b border-border flex-shrink-0 [&_input]:!text-[14px] [&>div>span]:!text-[14px]">
        <RecipientField chips={to} onChange={setTo} placeholder={t('mail_add_recipient')} />
        <div className="flex items-center gap-2 flex-shrink-0 mt-0.5">
          {!showCc && (
            <button onClick={() => setShowCc(true)} className="text-[14px] text-text-tertiary hover:text-primary whitespace-nowrap">
              CC
            </button>
          )}
          {!showBcc && (
            <button onClick={() => setShowBcc(true)} className="text-[14px] text-text-tertiary hover:text-primary whitespace-nowrap">
              {t('mail_bcc', { defaultValue: 'CCi' })}
            </button>
          )}
        </div>
      </div>

      {/* ── CC ───────────────────────────────────────────────────────────────── */}
      {showCc && (
        <div className="flex items-start px-4 py-2 border-b border-border flex-shrink-0 [&_input]:!text-[14px] [&>div>span]:!text-[14px]">
          <span className="text-[14px] text-text-tertiary mr-3 mt-0.5 flex-shrink-0">CC</span>
          <RecipientField chips={cc} onChange={setCc} placeholder={t('to_add')} />
        </div>
      )}

      {/* ── CCi (destinataires cachés) ──────────────────────────────────────── */}
      {showBcc && (
        <div className="flex items-start px-4 py-2 border-b border-border flex-shrink-0 [&_input]:!text-[14px] [&>div>span]:!text-[14px]">
          <span className="text-[14px] text-text-tertiary mr-3 mt-0.5 flex-shrink-0">{t('mail_bcc', { defaultValue: 'CCi' })}</span>
          <RecipientField chips={bcc} onChange={setBcc} placeholder={t('to_add')} />
        </div>
      )}

      {/* ── Objet ────────────────────────────────────────────────────────────── */}
      <div className="px-4 py-2.5 border-b border-border flex-shrink-0">
        <input
          type="text"
          value={subject}
          onChange={e => setSubject(e.target.value)}
          placeholder={t('subject')}
          className="w-full text-[14px] outline-none bg-transparent text-text-primary placeholder:text-text-tertiary"
        />
      </div>

      {/* ── Body (contenteditable) ────────────────────────────────────────────── */}
      {/* Wrapped so the placeholder overlay can sit at the top of the typing area
          (above the signature/quote, where the caret is) — the CSS `empty:before`
          pseudo can't do that once a signature is present. */}
      <div className="relative flex-1 flex flex-col min-h-0">
        <div
          ref={mentions.attachBody}
          contentEditable
          suppressContentEditableWarning
          spellCheck={spellcheck}
          onInput={mentions.onInput}
          onKeyDown={mentions.onKeyDown}
          onKeyUp={mentions.onKeyUp}
          onMouseUp={mentions.onMouseUp}
          className="flex-1 px-4 py-3 text-sm text-text-primary outline-none overflow-y-auto overflow-x-auto break-words
                     [&_img]:max-w-full [&_img]:h-auto"
          style={{ lineHeight: '1.6' }}
          onPaste={e => {
            // Cross-module data paste: insert a sanitizer-proof HTML block
            // instead of the envelope's plain-text fallback.
            const env = readKubunoData(e.clipboardData)
            if (!env) return
            e.preventDefault()
            document.execCommand('insertHTML', false, kubunoDataToEmailHtml(env))
            refreshPlaceholder()
          }}
        />
        {showPlaceholder && (
          <div className="pointer-events-none absolute left-4 top-3 text-sm text-text-tertiary" style={{ lineHeight: '1.6' }}>
            {t('body')}
          </div>
        )}
        {mentions.mentionList}
      </div>

      {/* ── Pièces jointes ────────────────────────────────────────────────────── */}
      <AttachmentBar attachments={atts.attachments} onRemove={atts.remove} objectUrl={atts.objectUrl} />

      {/* ── Format toolbar ───────────────────────────────────────────────────── */}
      {showFmt && !plainText && (
        <ComposeFormatToolbar
          exec={exec}
          fontName={fontName} setFontName={setFontName}
          fontSz={fontSz} setFontSz={setFontSz}
          applyFontSizePx={applyFontSizePx}
          focusBody={() => bodyRef.current?.focus()}
          setAlignMenu={setAlignMenu}
        />
      )}

      {/* ── Scheduled-send error (immediate send surfaces via the toast) ─────── */}
      {sendMut.isError && (
        <div className="px-4 py-2 border-t border-border text-sm text-danger bg-danger/5 flex-shrink-0">
          {apiErrorMessage(sendMut.error, t('mail_send_failed', { defaultValue: "L'envoi a échoué." }))}
        </div>
      )}

      {/* ── Action bar ───────────────────────────────────────────────────────── */}
      <ComposeActionBar
        onSendNow={sendNow}
        onSchedule={iso => sendMut.mutate(iso)}
        sendDisabled={!to.length || !accountId}
        sendPending={sendMut.isPending}
        scheduleAutoOpen={scheduleOnMountRef.current}
        plainText={plainText}
        showFmt={showFmt} setShowFmt={setShowFmt}
        exec={exec}
        fileRef={fileRef} onPickFiles={onPickFiles}
        insertLink={insertLink} insertImageUrl={insertImageUrl}
        confidential={confidential} setConfidential={setConfidential}
        gpgEnabled={gpgEnabled} pgpSign={pgpSign} pgpEncrypt={pgpEncrypt} secItems={secItems}
        sigItems={sigItems} moreItems={moreItems}
        draftStatus={draft.status}
        onDiscard={() => { void draft.discard(); setComposeOpen(false) }}
      />

      {alignMenu && <MenuDropdown items={alignItems} pos={{ ...alignMenu, minWidth: 176 }} onClose={() => setAlignMenu(null)} />}

      {/* « Ajout des fichiers… » : uploads of oversized files to Drive in flight. */}
      <LargeFilesModal
        files={atts.pendingUploads}
        maxMb={maxMb}
        onCancelOne={atts.remove}
        onCancelAll={() => atts.pendingUploads.forEach(f => atts.remove(f.id))}
      />
    </div>
  )

  // Title-bar controls: minimize + maximize/restore (hidden on mobile, where the
  // composer is already full screen).
  const titleActions = isMobile ? undefined : (
    <div className="flex items-center gap-0.5">
      <button
        onClick={() => setMinimized(true)}
        className="p-1.5 rounded-lg text-text-tertiary hover:text-text-primary hover:bg-surface-2 transition-colors"
        title={t('mail_less', { defaultValue: 'Réduire' })}
      >
        <Minus size={15} />
      </button>
      <button
        onClick={() => setMaximized(v => !v)}
        className="p-1.5 rounded-lg text-text-tertiary hover:text-text-primary hover:bg-surface-2 transition-colors"
        title={maximized
          ? t('mail_exit_fullscreen', { defaultValue: 'Quitter le plein écran' })
          : t('mail_fullscreen', { defaultValue: 'Plein écran' })}
      >
        {maximized ? <Copy size={14} /> : <Square size={14} />}
      </button>
    </div>
  )

  // Full-screen mode (Gmail "Plein écran"): render the SAME composer body inside a
  // centered fixed panel instead of the floating window. Behaviour (editing,
  // autosave, sending) is identical — only the shell differs.
  if (maximized && !isMobile) {
    return (
      <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-6">
        <div className="w-full max-w-4xl h-full bg-white rounded-xl shadow-2xl flex flex-col overflow-hidden">
          <div className="flex items-center justify-between px-4 py-2 bg-surface-1 border-b border-border flex-shrink-0">
            <span className="text-sm font-medium text-text-primary truncate">{subject || t('new_message')}</span>
            <div className="flex items-center gap-0.5 flex-shrink-0">
              {titleActions}
              <button
                onClick={() => setComposeOpen(false)}
                className="p-1.5 rounded-lg text-text-tertiary hover:text-text-primary hover:bg-surface-2 transition-colors"
                title={t('mail_close', { defaultValue: 'Fermer' })}
              >
                <X size={15} />
              </button>
            </div>
          </div>
          {composerBody}
        </div>
      </div>
    )
  }

  return (
    <FloatingWindow
      title={t('new_message')}
      onClose={() => setComposeOpen(false)}
      defaultWidth={600}
      defaultHeight={520}
      minWidth={MIN_W}
      minHeight={MIN_H}
      resizable
      titleActions={titleActions}
    >
      {composerBody}
    </FloatingWindow>
  )
}
