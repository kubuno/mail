import { useCallback, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import DOMPurify from 'dompurify'
import {
  Reply, Loader2, Send, Paperclip, Smile, Trash2,
  Undo2, Redo2, Bold, Italic, Underline, Strikethrough,
  AlignLeft, AlignCenter, AlignRight,
  List, ListOrdered, Indent, Outdent, Eraser, Type, Link, Image,
  ShieldCheck, PenLine, Check,
} from 'lucide-react'
import { Button, Dropdown, MenuDropdown, useIsMobile, serializeMentions, type MenuItem, type MenuDropdownPos } from '@ui'
import { mailApi, EmailMessage, apiErrorMessage } from '../api'
import { loadSignatures, defaultSignatureId, resolveSignatureId } from '../signatures'
import { longDateTime , replyAllExtras } from './helpers'
import { activeSenders, defaultReplySender } from '../senderSelection'
import { useMailStore } from '../store'
import { RecipientField } from '../AddressSuggest'
import { readKubunoData, kubunoDataToEmailHtml } from '../kubunoData'
import { useDraftAutosave, type DraftSnapshot } from './useDraftAutosave'
import { loadPrefs } from '../settings/GeneralTab'
import { signatureBlock, shouldShowPlaceholder, placeCaretAtStart, QUOTE_ATTR, appendDriveLinksHtml } from './composeContent'
import { useComposeAttachments } from './composeAttachments'
import { useComposeMentions } from './useComposeMentions'
import AttachmentBar from './AttachmentChip'
import LargeFilesModal from './LargeFilesModal'
import { useMaxMessageSize } from './useMaxMessageSize'

// ── Inline compose (reply / forward) ─────────────────────────────────────────

function ToolBtn({ onClick, title, children }: { onClick: () => void; title: string; children: React.ReactNode }) {
  return (
    <button
      onMouseDown={e => { e.preventDefault(); onClick() }}
      title={title}
      className="w-7 h-7 flex items-center justify-center rounded hover:bg-surface-2 text-text-secondary transition-colors flex-shrink-0"
    >
      {children}
    </button>
  )
}

export default function InlineCompose({
  mode, message, onSent, onCancel, replyAll = false,
}: {
  mode:     'reply' | 'forward'
  message:  EmailMessage
  onSent:   () => void
  onCancel: () => void
  /** Reply to every recipient: original To (minus us) go to Cc. */
  replyAll?: boolean
}) {
  const { t, i18n } = useTranslation('mail')
  const { accounts } = useMailStore()
  const qc = useQueryClient()
  const bodyRef   = useRef<HTMLDivElement>(null)
  const composeRef = useRef<HTMLDivElement>(null)
  const fileRef   = useRef<HTMLInputElement>(null)

  // Attachments: base64-encoded locally, carried in the /send payload (no upload
  // endpoint yet). Shared hook drives progress / cancel / preview.
  // Files over the admin max message size are uploaded to Drive as public links
  // instead of being base64-encoded into /send (Gmail parity).
  const { maxMb, maxBytes } = useMaxMessageSize()
  const atts = useComposeAttachments(undefined, { maxBytes })
  const driveLinksIntro = t('mail_drive_links_intro', { defaultValue: 'Fichiers partagés via Drive :' })
  const onPickFiles = (files: FileList | null) => {
    atts.addFiles(files)
    if (fileRef.current) fileRef.current.value = ''
  }

  // Drag & drop of files onto the composer (Gmail-style « Déposez ici »). Only a
  // file drag lights up the zone; dragging text inside the editor is ignored. A
  // depth counter keeps the overlay stable across child dragenter/dragleave.
  const [dragOver, setDragOver] = useState(false)
  const dragDepth = useRef(0)
  const isFileDrag = (e: React.DragEvent) => e.dataTransfer?.types?.includes('Files')
  const onDragEnter = (e: React.DragEvent) => {
    if (!isFileDrag(e)) return
    e.preventDefault(); dragDepth.current += 1; setDragOver(true)
  }
  const onDragOver = (e: React.DragEvent) => {
    if (!isFileDrag(e)) return
    e.preventDefault(); e.dataTransfer.dropEffect = 'copy'
  }
  const onDragLeave = (e: React.DragEvent) => {
    if (!isFileDrag(e)) return
    dragDepth.current = Math.max(0, dragDepth.current - 1)
    if (dragDepth.current === 0) setDragOver(false)
  }
  const onDropFiles = (e: React.DragEvent) => {
    if (!isFileDrag(e)) return
    e.preventDefault(); dragDepth.current = 0; setDragOver(false)
    if (e.dataTransfer.files?.length) atts.addFiles(e.dataTransfer.files)
  }
  // Mobile: the format bar starts hidden (the "Aa" toggle reveals a reduced
  // one-line set) and the non-wired insert buttons are dropped.
  const isMobile = useIsMobile()

  // Only active accounts can send: a deactivated one (its mailbox was deleted)
  // must not appear as a possible sender.
  const activeAccounts = activeSenders(accounts)
  // Reply from the account that RECEIVED the message when it is still active
  // (fallback: the default active account, then the first). A "From" row lets
  // the user switch when several accounts are configured.
  const receivingAccount = defaultReplySender(accounts, message.account_id)
  const [fromId, setFromId] = useState(receivingAccount?.id ?? '')
  const accountId = (activeAccounts.some(a => a.id === fromId) ? fromId : receivingAccount?.id) ?? ''
  const [to,        setTo]        = useState<{ email: string; name?: string }[]>(
    mode === 'reply' ? [{ email: message.from_email, name: message.from_name ?? undefined }] : []
  )
  const [cc,         setCc]       = useState<{ email: string; name?: string }[]>(() => {
    if (mode !== 'reply' || !replyAll) return []
    return replyAllExtras(message, accounts.map(a => a.email_address))
  })
  const [showCc,     setShowCc]   = useState(mode === 'reply' && replyAll)
  const [showFormat, setShowFormat] = useState(() => !isMobile)

  // Gmail-style placeholder: visible while the body holds only the auto-inserted
  // signature and the quoted message; hidden as soon as the user types above them.
  const [showPlaceholder, setShowPlaceholder] = useState(true)
  const refreshPlaceholder = useCallback(() => {
    setShowPlaceholder(shouldShowPlaceholder(bodyRef.current))
  }, [])

  // Injects the quote INTO the editor (and therefore into the sent mail): full
  // "Forwarded message" header when forwarding, "On …, X wrote:" + blockquote
  // when replying. The old "…" preview block was never transmitted → a forward
  // literally went out empty.
  useEffect(() => {
    const el = bodyRef.current
    if (!el) return
    const esc = (s: string) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
    const orig = message.body_html
      ? DOMPurify.sanitize(message.body_html)
      : `<pre style="white-space:pre-wrap;font-family:inherit">${esc(message.body_text ?? '')}</pre>`
    const date = longDateTime(new Date(message.sent_at ?? message.received_at), i18n.language)
    const from = message.from_name ? `${message.from_name} <${message.from_email}>` : message.from_email
    const toLine = Array.isArray(message.to_addresses)
      ? message.to_addresses.map(a => a.name ? `${a.name} <${a.email}>` : a.email).join(', ')
      : ''
    // The quote is wrapped in a marked container so it can be excluded from the
    // "logically empty" test (placeholder + phantom-draft prevention).
    let quote: string
    if (mode === 'forward') {
      quote =
        `<div ${QUOTE_ATTR}><br><br><div>---------- ${t('mail_forwarded_header', { defaultValue: 'Message transféré' })} ----------<br>` +
        `${t('mail_fwd_from', { defaultValue: 'De' })} : ${esc(from)}<br>` +
        `${t('mail_fwd_date', { defaultValue: 'Date' })} : ${esc(date)}<br>` +
        `${t('mail_fwd_subject', { defaultValue: 'Objet' })} : ${esc(message.subject)}<br>` +
        (toLine ? `${t('mail_fwd_to', { defaultValue: 'À' })} : ${esc(toLine)}<br>` : '') +
        `<br></div>${orig}</div>`
    } else {
      quote =
        `<div ${QUOTE_ATTR}><br><br><div style="color:#5f6368;font-size:12px">${esc(t('mail_reply_header', { defaultValue: 'Le {{date}}, {{from}} a écrit :', date, from }))}</div>` +
        `<blockquote style="border-left:2px solid #dadce0;padding-left:12px;margin:4px 0 0;color:#5f6368">${orig}</blockquote></div>`
    }
    // Prefill the default reply/forward signature (Gmail-style), ABOVE the quote.
    // Resolve in order: the receiving address's per-address default → the global
    // « reply/forward » preference → the single default signature.
    const sigId = resolveSignatureId(receivingAccount?.email_address, 'reply', loadPrefs().signatureReply)
    const sig = loadSignatures().find(s => s.id === sigId)
    el.innerHTML = (sig ? signatureBlock(sig.html) : '') + quote
    // Caret at the top: we write ABOVE the signature and the quote, like Gmail.
    placeCaretAtStart(el)
    refreshPlaceholder()
  }, []) // eslint-disable-line react-hooks/exhaustive-deps

  // Subject is prefilled (Re:/Fwd:) but editable — a reply's subject is the
  // user's to change like any other.
  const [subject, setSubject] = useState(mode === 'reply'
    ? (message.subject.startsWith('Re:') ? message.subject : `Re: ${message.subject}`)
    : (message.subject.startsWith('Fwd:') ? message.subject : `Fwd: ${message.subject}`))

  const exec = (cmd: string, value?: string) => {
    bodyRef.current?.focus()
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    ;(document as any).execCommand(cmd, false, value ?? undefined)
  }

  // ── OpenPGP: sign / encrypt (mirrors ComposeWindow) ──────────────────────────
  // The control only appears when the instance admin enabled the feature.
  const [pgpSign,    setPgpSign]    = useState(false)
  const [pgpEncrypt, setPgpEncrypt] = useState(false)
  const [secMenu,    setSecMenu]    = useState<MenuDropdownPos | null>(null)
  const { data: pgpStatus } = useQuery({ queryKey: ['mail-pgp-status'], queryFn: mailApi.pgpStatus })
  const gpgEnabled = !!pgpStatus?.enabled
  const secItems: MenuItem[] = [
    { type: 'action', label: t('mail_pgp_sign', { defaultValue: 'Signer numériquement' }),
      icon: pgpSign ? <Check size={15} /> : <span className="inline-block w-[15px]" />, onClick: () => setPgpSign(v => !v) },
    { type: 'action', label: t('mail_pgp_encrypt', { defaultValue: 'Chiffrer (OpenPGP)' }),
      icon: pgpEncrypt ? <Check size={15} /> : <span className="inline-block w-[15px]" />, onClick: () => setPgpEncrypt(v => !v) },
  ]

  // ── Signatures menu (pen icon), Gmail-style ──────────────────────────────────
  const [sigMenu, setSigMenu] = useState<MenuDropdownPos | null>(null)
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

  // ── Auto-save the reply/forward as a draft (Gmail-style) ─────────────────────
  const getSnapshot = useCallback((): DraftSnapshot | null => {
    if (!accountId) return null
    return {
      account_id:    accountId,
      to_addresses:  to,
      cc_addresses:  cc,
      bcc_addresses: [],
      subject,
      body_html:     bodyRef.current?.innerHTML ?? '',
      // Ties the draft to the message it answers — it shows in the thread AND
      // in the Drafts folder, as Gmail does.
      reply_to_id:   mode === 'reply' ? message.id : undefined,
    }
  }, [accountId, to, cc, subject, mode, message.id])
  const draft = useDraftAutosave(getSnapshot)
  useEffect(() => { draft.touch() }, [to, cc, subject]) // eslint-disable-line react-hooks/exhaustive-deps

  // @mention in the body (dropdown of contacts, chip insertion, mailto on send).
  // A body mutation (typing OR a chip in/out) touches autosave + the placeholder.
  const mentions = useComposeMentions(bodyRef, useCallback(() => {
    draft.touch(); refreshPlaceholder()
  }, [draft, refreshPlaceholder]))
  // Baseline once the quote/forward header is in place (the effect above injected
  // it on mount): a reply opened but not typed into must leave no draft.
  useEffect(() => {
    const id = requestAnimationFrame(() => draft.captureBaseline())
    return () => cancelAnimationFrame(id)
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const sendMut = useMutation({
    mutationFn: () => {
      draft.markSent()
      return mailApi.sendMail({
        account_id:   accountId,
        to_addresses: to,
        cc_addresses: cc.length ? cc : undefined,
        subject,
        body_html:    appendDriveLinksHtml(serializeMentions(bodyRef.current?.innerHTML ?? '', 'mailto'), atts.driveLinks(), driveLinksIntro),
        reply_to_id:  mode === 'reply' ? message.id : undefined,
        sign:         pgpSign || undefined,
        encrypt:      pgpEncrypt || undefined,
        draft_id:     draft.draftId() ?? undefined,
        attachments:  atts.payload().length ? atts.payload() : undefined,
      })
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['mail-threads'] })
      qc.invalidateQueries({ queryKey: ['mail-counts'] })
      qc.invalidateQueries({ queryKey: ['mail-drafts'] })
      onSent()
    },
  })

  return (
    <div
      ref={composeRef}
      className="relative border border-border rounded-2xl bg-white shadow-sm overflow-hidden"
      onDragEnter={onDragEnter}
      onDragOver={onDragOver}
      onDragLeave={onDragLeave}
      onDrop={onDropFiles}
    >
      {/* ── Drop zone (glisser-déposer de fichiers) ───────────────────────── */}
      {dragOver && (
        <div className="pointer-events-none absolute inset-2 z-30 flex items-center justify-center rounded-xl border-2 border-dashed border-primary bg-white/70 backdrop-blur-[2px]">
          <span className="text-lg font-medium text-text-tertiary select-none">
            {t('mail_drop_files', { defaultValue: 'Déposez les fichiers ici' })}
          </span>
        </div>
      )}

      {/* ── To field (autocompletion: mail index + contacts) ──────────────── */}
      <div className="flex items-start gap-2 px-4 py-2.5 border-b border-border">
        <Reply size={14} className="text-text-tertiary flex-shrink-0 mt-1" />
        <RecipientField chips={to} onChange={setTo} placeholder={t('mail_add_recipient')} />
        {!showCc && (
          <button onClick={() => setShowCc(true)} className="text-xs text-text-tertiary hover:text-primary whitespace-nowrap flex-shrink-0 mt-0.5">
            + CC
          </button>
        )}
      </div>

      {/* ── CC field ──────────────────────────────────────────────────────── */}
      {showCc && (
        <div className="flex items-start gap-2 px-4 py-2 border-b border-border">
          <span className="text-xs text-text-tertiary w-6 flex-shrink-0 mt-1">CC</span>
          <RecipientField chips={cc} onChange={setCc} placeholder={t('to_add')} />
        </div>
      )}

      {/* ── From field (account picker, only with several accounts) ───────── */}
      {activeAccounts.length > 1 && (
        <div className="flex items-center gap-2 px-4 py-1.5 border-b border-border">
          <span className="text-xs text-text-tertiary w-6 flex-shrink-0">{t('mail_filter_from')}</span>
          <Dropdown
            value={accountId}
            onChange={setFromId}
            options={activeAccounts.map(a => ({ value: a.id, label: `${a.name} <${a.email_address}>` }))}
            variant="ghost"
            height={24}
            fontSize={12}
          />
        </div>
      )}

      {/* ── Subject (editable, prefilled Re:/Fwd:) ────────────────────────── */}
      <div className="flex items-center gap-2 px-4 py-1.5 border-b border-border">
        <span className="text-xs text-text-tertiary w-6 flex-shrink-0">{t('subject')}</span>
        <input
          value={subject}
          onChange={e => setSubject(e.target.value)}
          placeholder={t('subject')}
          className="flex-1 text-xs text-text-primary outline-none bg-transparent placeholder:text-text-tertiary"
        />
      </div>

      {/* ── Body (contenteditable) ────────────────────────────────────────── */}
      {/* Wrapped so the placeholder overlay sits at the top of the typing area
          (above the signature/quote, where the caret is). */}
      <div className="relative">
        <div
          ref={mentions.attachBody}
          contentEditable
          suppressContentEditableWarning
          // overflow-x-auto + img clamp: the injected quote can carry fixed-width
          // email tables — they must scroll inside the card, not widen the page.
          className="px-4 py-3 min-h-[100px] text-xs text-text-primary outline-none overflow-x-auto break-words
                     [&_img]:max-w-full [&_img]:h-auto"
          style={{ lineHeight: '1.6' }}
          onInput={mentions.onInput}
          onKeyDown={mentions.onKeyDown}
          onKeyUp={mentions.onKeyUp}
          onMouseUp={mentions.onMouseUp}
          onPaste={e => {
            // Cross-module data paste (see kubunoData.kubunoDataToEmailHtml).
            const env = readKubunoData(e.clipboardData)
            if (!env) return
            e.preventDefault()
            document.execCommand('insertHTML', false, kubunoDataToEmailHtml(env))
            refreshPlaceholder()
          }}
        />
        {showPlaceholder && (
          <div className="pointer-events-none absolute left-4 top-3 text-xs text-text-tertiary" style={{ lineHeight: '1.6' }}>
            {t('body')}
          </div>
        )}
        {mentions.mentionList}
      </div>

      {/* ── Pièces jointes ────────────────────────────────────────────────── */}
      <AttachmentBar attachments={atts.attachments} onRemove={atts.remove} objectUrl={atts.objectUrl} />

      {/* ── Format toolbar ────────────────────────────────────────────────── */}
      {/* Mobile: reduced one-line set (B/I/U, lists, clear) — the full bar with
          font selects and alignment would wrap into several rows. */}
      {showFormat && isMobile && (
        <div className="flex items-center gap-0.5 px-3 py-1.5 border-t border-border bg-surface-1/40">
          <ToolBtn onClick={() => exec('bold')}          title={t('mail_bold')}><Bold size={14} /></ToolBtn>
          <ToolBtn onClick={() => exec('italic')}        title={t('mail_italic')}><Italic size={14} /></ToolBtn>
          <ToolBtn onClick={() => exec('underline')}     title={t('mail_underline')}><Underline size={14} /></ToolBtn>
          <div className="w-px h-4 bg-border mx-0.5" />
          <ToolBtn onClick={() => exec('insertOrderedList')}   title={t('mail_ordered_list')}><ListOrdered size={14} /></ToolBtn>
          <ToolBtn onClick={() => exec('insertUnorderedList')} title={t('mail_bullet_list')}><List size={14} /></ToolBtn>
          <div className="w-px h-4 bg-border mx-0.5" />
          <ToolBtn onClick={() => exec('removeFormat')} title={t('mail_clear_format')}><Eraser size={14} /></ToolBtn>
        </div>
      )}
      {showFormat && !isMobile && (
        <div className="flex items-center flex-wrap gap-0.5 px-3 py-1.5 border-t border-border bg-surface-1/40">
          <ToolBtn onClick={() => exec('undo')}   title={t('common_undo')}><Undo2 size={13} /></ToolBtn>
          <ToolBtn onClick={() => exec('redo')}   title={t('common_redo')}><Redo2 size={13} /></ToolBtn>
          <div className="w-px h-4 bg-border mx-0.5" />
          <select
            onMouseDown={e => e.stopPropagation()}
            onChange={e => { bodyRef.current?.focus(); exec('fontName', e.target.value) }}
            className="text-xs border border-border rounded px-1 py-0.5 outline-none text-text-secondary bg-white h-6"
          >
            <option value="sans-serif">{t('mail_font_sans')}</option>
            <option value="serif">{t('mail_font_serif')}</option>
            <option value="Georgia">Georgia</option>
            <option value="Arial">Arial</option>
            <option value="monospace">{t('mail_font_mono')}</option>
          </select>
          <select
            onMouseDown={e => e.stopPropagation()}
            onChange={e => { bodyRef.current?.focus(); exec('fontSize', e.target.value) }}
            className="text-xs border border-border rounded px-1 py-0.5 ml-0.5 outline-none text-text-secondary bg-white h-6 w-14"
          >
            {(['8','10','12','14','18','24','36']).map((size, i) => (
              <option key={size} value={String(i + 1)}>{size}</option>
            ))}
          </select>
          <div className="w-px h-4 bg-border mx-0.5" />
          <ToolBtn onClick={() => exec('bold')}          title={t('mail_bold')}><Bold size={13} /></ToolBtn>
          <ToolBtn onClick={() => exec('italic')}        title={t('mail_italic')}><Italic size={13} /></ToolBtn>
          <ToolBtn onClick={() => exec('underline')}     title={t('mail_underline')}><Underline size={13} /></ToolBtn>
          <ToolBtn onClick={() => exec('strikeThrough')} title={t('mail_strikethrough')}><Strikethrough size={13} /></ToolBtn>
          <div className="w-px h-4 bg-border mx-0.5" />
          <ToolBtn onClick={() => exec('justifyLeft')}   title={t('mail_align_left')}><AlignLeft size={13} /></ToolBtn>
          <ToolBtn onClick={() => exec('justifyCenter')} title={t('mail_align_center')}><AlignCenter size={13} /></ToolBtn>
          <ToolBtn onClick={() => exec('justifyRight')}  title={t('mail_align_right')}><AlignRight size={13} /></ToolBtn>
          <div className="w-px h-4 bg-border mx-0.5" />
          <ToolBtn onClick={() => exec('insertOrderedList')}   title={t('mail_ordered_list')}><ListOrdered size={13} /></ToolBtn>
          <ToolBtn onClick={() => exec('insertUnorderedList')} title={t('mail_bullet_list')}><List size={13} /></ToolBtn>
          <ToolBtn onClick={() => exec('indent')}  title={t('mail_indent')}><Indent size={13} /></ToolBtn>
          <ToolBtn onClick={() => exec('outdent')} title={t('mail_outdent')}><Outdent size={13} /></ToolBtn>
          <div className="w-px h-4 bg-border mx-0.5" />
          <ToolBtn onClick={() => exec('removeFormat')} title={t('mail_clear_format')}><Eraser size={13} /></ToolBtn>
        </div>
      )}

      {/* ── Send error (silent failures are the worst kind: surface it) ───── */}
      {sendMut.isError && (
        <div className="px-4 py-2 border-t border-border text-xs text-danger bg-danger/5">
          {apiErrorMessage(sendMut.error, t('mail_send_failed', { defaultValue: "L'envoi a échoué." }))}
        </div>
      )}

      {/* ── Bottom bar ────────────────────────────────────────────────────── */}
      <div className="flex items-center gap-1 px-4 py-2.5 border-t border-border">
        <Button
          size="sm"
          onClick={() => sendMut.mutate()}
          disabled={!to.length || !accountId || !subject.trim() || sendMut.isPending}
          icon={sendMut.isPending ? <Loader2 size={13} className="animate-spin" /> : <Send size={13} />}
          className="mr-1"
        >
          {t('mail_send')}
        </Button>
        <button
          onClick={() => setShowFormat(v => !v)}
          className={`p-1.5 rounded hover:bg-surface-2 transition-colors ${showFormat ? 'text-primary' : 'text-text-tertiary'}`}
          title={t('mail_formatting')}
        >
          <Type size={15} />
        </button>
        {gpgEnabled && (
          <button
            onClick={e => { const r = e.currentTarget.getBoundingClientRect(); setSecMenu(p => (p ? null : { top: r.top - 4, left: r.left })) }}
            className={`p-1.5 rounded hover:bg-surface-2 transition-colors ${(pgpSign || pgpEncrypt) ? 'text-primary' : 'text-text-tertiary'}`}
            title={t('mail_pgp_security', { defaultValue: 'Signer / Chiffrer (OpenPGP)' })}
          >
            <ShieldCheck size={15} />
          </button>
        )}
        <button
          onClick={e => { const r = e.currentTarget.getBoundingClientRect(); setSigMenu(p => (p ? null : { top: r.top - 4, left: r.left })) }}
          className="p-1.5 rounded hover:bg-surface-2 text-text-tertiary transition-colors"
          title={t('mail_signature', { defaultValue: 'Insérer une signature' })}
        >
          <PenLine size={15} />
        </button>
        {!isMobile && <>
          <button onClick={() => fileRef.current?.click()} className="p-1.5 rounded hover:bg-surface-2 text-text-tertiary" title={t('mail_attach_file')}><Paperclip size={15} /></button>
          <input ref={fileRef} type="file" multiple hidden onChange={e => onPickFiles(e.target.files)} />
          <button className="p-1.5 rounded hover:bg-surface-2 text-text-tertiary" title={t('mail_insert_link')}><Link size={15} /></button>
          <button className="p-1.5 rounded hover:bg-surface-2 text-text-tertiary" title={t('mail_insert_emoji')}><Smile size={15} /></button>
          <button className="p-1.5 rounded hover:bg-surface-2 text-text-tertiary" title={t('mail_insert_image')}><Image size={15} /></button>
        </>}
        <div className="flex-1" />
        {draft.status !== 'idle' && (
          <span className="text-[11px] text-text-tertiary mr-1 select-none">
            {draft.status === 'saving'
              ? t('mail_draft_saving', { defaultValue: 'Enregistrement…' })
              : t('mail_draft_saved',  { defaultValue: 'Brouillon enregistré' })}
          </span>
        )}
        <button
          onClick={() => { void draft.discard(); onCancel() }}
          className="p-1.5 rounded hover:bg-danger/10 hover:text-danger text-text-tertiary transition-colors"
          title={t('discard')}
        >
          <Trash2 size={15} />
        </button>
      </div>

      {secMenu && <MenuDropdown items={secItems} pos={{ ...secMenu, minWidth: 220 }} onClose={() => setSecMenu(null)} />}
      {sigMenu && <MenuDropdown items={sigItems} pos={{ ...sigMenu, minWidth: 200 }} onClose={() => setSigMenu(null)} />}

      {/* « Ajout des fichiers… » : uploads of oversized files to Drive in flight. */}
      <LargeFilesModal
        files={atts.pendingUploads}
        maxMb={maxMb}
        onCancelOne={atts.remove}
        onCancelAll={() => atts.pendingUploads.forEach(f => atts.remove(f.id))}
      />
    </div>
  )
}
