import { useEffect, useState, useRef, useMemo } from 'react'
import { isCoarsePointer } from './openable'
import { useSwipeActions } from './useSwipe'
import { useTranslation } from 'react-i18next'
import type { TFunction } from 'i18next'
import { useLocation } from 'react-router-dom'
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import {
  RefreshCw, ChevronLeft, ChevronRight, ChevronDown,
  Reply, Forward, Loader2, Mail as MailIcon, MailOpen, Star, Trash2,
  Paperclip, Download, FileText, MoreVertical, Smile, Send, X,
  AlertCircle, ShieldAlert, ShieldCheck, Flag, Filter, Languages, Printer, Code2,
  Archive, Clock, ExternalLink, Settings2, Bookmark, Ban,
  Columns2, Rows2, Square, BellOff, AlignJustify,
  Undo2, Redo2, Bold, Italic, Underline, Strikethrough,
  AlignLeft, AlignCenter, AlignRight,
  List, ListOrdered, Indent, Outdent, Eraser, Type, Link, Image, MoreHorizontal,
} from 'lucide-react'
import DOMPurify from 'dompurify'
import { mailApi, Thread, EmailMessage, Attachment } from './api'
import { Button, MenuDropdown, type MenuDropdownPos } from '@ui'
import { useMailStore } from './store'
import ComposeWindow from './ComposeWindow'
import MailSidebarBody from './MailSidebarBody'
import PdfViewerModal from './PdfViewerModal'
import { ScheduledView, SubscriptionsView } from './MailViews'
import { useUndoSendStore } from './undoSendStore'

// ── Helpers ───────────────────────────────────────────────────────────────────

function formatDate(s: string, t: TFunction, lang: string) {
  const d = new Date(s)
  const now = new Date()
  const diffMs = now.getTime() - d.getTime()
  const diffMin = Math.floor(diffMs / 60_000)
  if (diffMin < 60)   return t('mail_ago_minutes', { count: diffMin })
  const diffH = Math.floor(diffMin / 60)
  if (diffH < 24)     return t('mail_ago_hours', { count: diffH })
  if (d.toDateString() === now.toDateString()) {
    return d.toLocaleTimeString(lang, { hour: '2-digit', minute: '2-digit' })
  }
  if (d.getFullYear() === now.getFullYear()) {
    return d.toLocaleDateString(lang, { month: 'short', day: 'numeric' })
  }
  return d.toLocaleDateString(lang, { month: 'short', day: 'numeric', year: 'numeric' })
}

function folderFromPath(pathname: string): string {
  const seg = pathname.replace(/^\/mail\/?/, '').split('/')[0]
  return seg || 'inbox'
}

// ── Visionneuse HTML d'email (Shadow DOM, SANS iframe) ────────────────────────
// Stratégie pour appliquer COMPLÈTEMENT le CSS de l'email sans parasites :
//  1. Shadow DOM → les styles de l'email ne fuient PAS vers l'app (encapsulation).
//  2. `:host { all: initial }` → coupe l'HÉRITAGE des styles de l'app (police,
//     couleur, line-height, text-align…) qui, lui, traverse la frontière shadow.
//     C'est le « reset sur la plupart des propriétés » qui exclut les parasites.
//  3. On CONSERVE l'élément <body> de l'email (pas seulement son contenu) pour que
//     ses règles `body { … }` et son style inline / bgcolor s'appliquent vraiment.
//  4. DOMPurify neutralise scripts, gestionnaires d'événements et protocoles
//     dangereux, tout en PRÉSERVANT <style> et les attributs `style` (le CSS).

// Sanitisation : on garde tout le CSS, on retire seulement ce qui est dangereux.
function sanitizeEmailHtml(html: string): string {
  return DOMPurify.sanitize(html, {
    WHOLE_DOCUMENT: true,                 // conserve <html>/<head>/<body> + <style> du head
    ADD_TAGS: ['style'],
    FORBID_TAGS: ['script', 'iframe', 'object', 'embed', 'base', 'meta', 'link', 'form'],
    FORBID_ATTR: ['ping'],
    ALLOW_UNKNOWN_PROTOCOLS: false,
  })
}

// Construit les nœuds DOM de l'email : les <style> (head + body) et l'élément <body>
// COMPLET. On renvoie de vrais nœuds (pas une chaîne) car injecter `<body>` via
// innerHTML le ferait SUPPRIMER par l'analyseur de fragment (html/head/body ne sont
// insérés qu'en contexte document). En APPENDANT l'élément <body>, il survit et les
// sélecteurs `body { … }` de l'email — très courants — s'appliquent vraiment.
function buildEmailNodes(html: string): { styles: HTMLStyleElement[]; body: HTMLElement } {
  const clean = sanitizeEmailHtml(html)
  const isDoc = /<html[\s>]/i.test(clean) || /^\s*<!doctype/i.test(clean)
  const doc = new DOMParser().parseFromString(isDoc ? clean : `<body>${clean}</body>`, 'text/html')

  // <style> du document entier (certains clients les placent dans le body) — détachés
  // pour les ré-insérer en tête du shadow (avant le <body>), sans doublon.
  const styles = Array.from(doc.querySelectorAll('style'))
  styles.forEach(s => s.remove())
  return { styles, body: doc.body }
}

// CSS de base injecté AVANT le CSS de l'email (priorité à l'email via la cascade).
const BASE_CSS = `
  /* (2) Reset radical : neutralise l'heritage des styles de l'app dans le shadow.
     all:initial ne touche PAS direction/unicode-bidi (exclus par la spec). */
  :host {
    all: initial;
    display: block;
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Arial, sans-serif;
    font-size: 14px;
    line-height: 1.6;
    color: #202124;
    text-align: left;
    background: #ffffff;
    padding: 16px;
  }

  *, *::before, *::after { box-sizing: border-box; }

  /* Le <body> de l'email est conservé : marges/typo par défaut, l'email peut redéfinir. */
  body {
    margin: 0;
    overflow-wrap: break-word;
    font-family: inherit;
    font-size: inherit;
    line-height: inherit;
    color: inherit;
  }
  p, div, span, td, th, table, ul, ol, li,
  h1, h2, h3, h4, h5, h6, blockquote, pre, figure { margin: 0; padding: 0; }

  /* Images responsives */
  img { max-width: 100%; height: auto; }

  /* Liens (l'email peut surcharger) */
  a { color: #1a73e8; text-decoration: underline; }
  a:hover { color: #1557b0; }

  /* Tables : éviter le débordement horizontal */
  table { border-collapse: collapse; max-width: 100%; }
  td, th { vertical-align: top; }

  /* Contenu cité */
  blockquote {
    border-left: 3px solid #dadce0;
    padding-left: 12px;
    margin: 8px 0 8px 4px;
    color: #5f6368;
  }

  /* Texte préformaté */
  pre, code {
    font-family: 'Google Sans Mono', 'Fira Code', monospace;
    font-size: 13px;
    background: #f1f3f4;
    border-radius: 4px;
    padding: 2px 4px;
    white-space: pre-wrap;
    word-break: break-word;
  }
  pre { padding: 12px; }

  hr { border: none; border-top: 1px solid #dadce0; margin: 12px 0; }
`

function EmailHtmlView({ html }: { html: string }) {
  const ref = useRef<HTMLDivElement>(null)

  useEffect(() => {
    const el = ref.current
    if (!el || !html) return

    const shadow = el.shadowRoot ?? el.attachShadow({ mode: 'open' })
    shadow.replaceChildren()

    // 1. CSS de base (reset anti-parasites + défauts email).
    const base = document.createElement('style')
    base.textContent = BASE_CSS
    shadow.appendChild(base)

    // 2. <style> de l'email puis son <body> COMPLET (append → l'élément survit,
    //    donc les règles `body { … }` et les styles inline / bgcolor s'appliquent).
    const { styles, body } = buildEmailNodes(html)
    styles.forEach(s => shadow.appendChild(s))
    shadow.appendChild(body)

    // Ouvrir les liens dans un nouvel onglet (les liens de l'email ne doivent pas
    // naviguer dans l'app Kubuno)
    shadow.querySelectorAll('a[href]').forEach(a => {
      const href = a.getAttribute('href') ?? ''
      if (href.startsWith('http') || href.startsWith('//')) {
        a.setAttribute('target', '_blank')
        a.setAttribute('rel', 'noopener noreferrer')
      }
    })
  }, [html])

  return <div ref={ref} />
}

// ── Message actions menu ──────────────────────────────────────────────────────

type LucideIcon = React.ComponentType<{ size?: number; className?: string }>
type MenuItem = {
  icon:    LucideIcon
  label:   string
  onClick: () => void
  danger?: boolean
}

function MessageActionsMenu({
  anchorRect, onClose, onReply, onForward, onDelete, onMarkUnread, onBlock,
}: {
  anchorRect:   DOMRect
  onClose:      () => void
  onReply:      () => void
  onForward:    () => void
  onDelete:     () => void
  onMarkUnread: () => void
  onBlock:      () => void
}) {
  const { t } = useTranslation('mail')
  const menuW = 285
  const top   = anchorRect.bottom + 4
  const right = Math.max(8, window.innerWidth - anchorRect.right)

  const groups: MenuItem[][] = [
    [
      { icon: Reply,       label: t('mail_reply'),             onClick: () => { onReply();      onClose() } },
      { icon: Forward,     label: t('mail_forward'),           onClick: () => { onForward();    onClose() } },
    ],
    [
      { icon: Trash2,      label: t('delete'),                 onClick: () => { onDelete();     onClose() }, danger: true },
      { icon: MailOpen,    label: t('mail_mark_unread'),       onClick: () => { onMarkUnread(); onClose() } },
    ],
    [
      { icon: AlertCircle, label: t('spam_report'),            onClick: onClose },
      { icon: Ban,         label: t('block_sender', { defaultValue: 'Bloquer l\'expéditeur' }), onClick: () => { onBlock(); onClose() } },
      { icon: ShieldAlert, label: t('mail_report_phishing'),   onClick: onClose },
      { icon: Flag,        label: t('mail_report_illegal'),    onClick: onClose },
    ],
    [
      { icon: Filter,    label: t('mail_filter_similar'),  onClick: onClose },
      { icon: Languages, label: t('mail_translate'),       onClick: onClose },
      { icon: Printer,   label: t('print'), onClick: () => { window.print(); onClose() } },
      { icon: Download,  label: t('mail_download_message'), onClick: onClose },
      { icon: Code2,     label: t('mail_show_original'),   onClick: onClose },
    ],
  ]

  return (
    <>
      <div className="fixed inset-0 z-[9998]" onClick={onClose} />
      <div
        className="fixed z-[9999] bg-white rounded-2xl shadow-2xl border border-border overflow-hidden py-1.5"
        style={{ top, right, width: menuW }}
        onClick={e => e.stopPropagation()}
      >
        {groups.map((group, gi) => (
          <div key={gi}>
            {gi > 0 && <div className="h-px bg-border mx-2 my-1.5" />}
            {group.map(({ icon: Icon, label, onClick, danger }) => (
              <button
                key={label}
                onClick={onClick}
                className={`w-full flex items-center gap-3 px-4 py-2.5 text-sm hover:bg-surface-1 transition-colors text-left ${
                  danger ? 'text-danger' : 'text-text-primary'
                }`}
              >
                <Icon size={16} className={`flex-shrink-0 ${danger ? 'text-danger' : 'text-text-secondary'}`} />
                {label}
              </button>
            ))}
          </div>
        ))}
      </div>
    </>
  )
}

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

function InlineCompose({
  mode, message, onSent, onCancel,
}: {
  mode:     'reply' | 'forward'
  message:  EmailMessage
  onSent:   () => void
  onCancel: () => void
}) {
  const { t } = useTranslation('mail')
  const { accounts } = useMailStore()
  const qc = useQueryClient()
  const bodyRef   = useRef<HTMLDivElement>(null)
  const composeRef = useRef<HTMLDivElement>(null)

  const defaultAccount = accounts.find(a => a.is_default) ?? accounts[0]
  const accountId = defaultAccount?.id ?? ''
  const [toInput,   setToInput]   = useState('')
  const [to,        setTo]        = useState<{ email: string; name?: string }[]>(
    mode === 'reply' ? [{ email: message.from_email, name: message.from_name ?? undefined }] : []
  )
  const [cc,         setCc]       = useState<{ email: string }[]>([])
  const [ccInput,    setCcInput]  = useState('')
  const [showCc,     setShowCc]   = useState(false)
  const [showFormat, setShowFormat] = useState(true)
  const [showQuote,  setShowQuote]  = useState(false)

  useEffect(() => { bodyRef.current?.focus() }, [])

  const subject = mode === 'reply'
    ? (message.subject.startsWith('Re:') ? message.subject : `Re: ${message.subject}`)
    : (message.subject.startsWith('Fwd:') ? message.subject : `Fwd: ${message.subject}`)

  const exec = (cmd: string, value?: string) => {
    bodyRef.current?.focus()
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    ;(document as any).execCommand(cmd, false, value ?? undefined)
  }

  const addTo = () => {
    const v = toInput.trim()
    if (v && v.includes('@')) { setTo(prev => [...prev, { email: v }]); setToInput('') }
  }
  const addCc = () => {
    const v = ccInput.trim()
    if (v && v.includes('@')) { setCc(prev => [...prev, { email: v }]); setCcInput('') }
  }

  const sendMut = useMutation({
    mutationFn: () => mailApi.sendMail({
      account_id:   accountId,
      to_addresses: to,
      cc_addresses: cc.length ? cc : undefined,
      subject,
      body_html:    bodyRef.current?.innerHTML ?? '',
      reply_to_id:  mode === 'reply' ? message.id : undefined,
    }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['mail-threads'] })
      onSent()
    },
  })

  return (
    <div ref={composeRef} className="border border-border rounded-2xl bg-white shadow-sm overflow-hidden">

      {/* ── To field ──────────────────────────────────────────────────────── */}
      <div className="flex items-start gap-2 px-4 py-2.5 border-b border-border">
        <Reply size={14} className="text-text-tertiary flex-shrink-0 mt-1" />
        <div className="flex-1 flex flex-wrap gap-1 items-center min-w-0">
          {to.map((a, i) => (
            <span key={i} className="flex items-center gap-1 bg-primary/10 text-primary text-xs px-2 py-0.5 rounded-full max-w-xs truncate">
              {a.name ? `${a.name} <${a.email}>` : a.email}
              <button onClick={() => setTo(prev => prev.filter((_, idx) => idx !== i))}><X size={9} /></button>
            </span>
          ))}
          <input
            type="email"
            value={toInput}
            onChange={e => setToInput(e.target.value)}
            onKeyDown={e => { if (e.key === 'Enter' || e.key === ',') { e.preventDefault(); addTo() } }}
            onBlur={addTo}
            placeholder={to.length ? '' : t('mail_add_recipient')}
            className="flex-1 min-w-28 text-sm outline-none bg-transparent text-text-primary placeholder:text-text-tertiary"
          />
        </div>
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
          <div className="flex-1 flex flex-wrap gap-1 items-center">
            {cc.map((a, i) => (
              <span key={i} className="flex items-center gap-1 bg-surface-2 text-text-secondary text-xs px-2 py-0.5 rounded-full">
                {a.email}
                <button onClick={() => setCc(prev => prev.filter((_, idx) => idx !== i))}><X size={9} /></button>
              </span>
            ))}
            <input
              type="email"
              value={ccInput}
              onChange={e => setCcInput(e.target.value)}
              onKeyDown={e => { if (e.key === 'Enter' || e.key === ',') { e.preventDefault(); addCc() } }}
              onBlur={addCc}
              placeholder={t('to_add')}
              className="flex-1 min-w-24 text-sm outline-none bg-transparent text-text-primary placeholder:text-text-tertiary"
            />
          </div>
        </div>
      )}

      {/* ── Body (contenteditable) ────────────────────────────────────────── */}
      <div
        ref={bodyRef}
        contentEditable
        suppressContentEditableWarning
        data-placeholder={t('body')}
        className="px-4 py-3 min-h-[100px] text-sm text-text-primary outline-none empty:before:content-[attr(data-placeholder)] empty:before:text-text-tertiary"
        style={{ lineHeight: '1.6' }}
      />

      {/* ── Quoted message ────────────────────────────────────────────────── */}
      {(message.body_html || message.body_text) && (
        <div className="px-4 pb-1">
          <button
            onClick={() => setShowQuote(v => !v)}
            className="text-text-tertiary hover:text-text-primary p-0.5 rounded"
            title={showQuote ? t('mail_hide_quoted') : t('mail_show_quoted')}
          >
            <MoreHorizontal size={16} />
          </button>
          {showQuote && (
            <div className="mt-1.5 border-l-2 border-border pl-3 opacity-70">
              {message.body_html
                ? <EmailHtmlView html={message.body_html} />
                : <pre className="text-xs text-text-tertiary whitespace-pre-wrap">{message.body_text}</pre>}
            </div>
          )}
        </div>
      )}

      {/* ── Format toolbar ────────────────────────────────────────────────── */}
      {showFormat && (
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

      {/* ── Bottom bar ────────────────────────────────────────────────────── */}
      <div className="flex items-center gap-1 px-4 py-2.5 border-t border-border">
        <Button
          size="sm"
          onClick={() => sendMut.mutate()}
          disabled={!to.length || !accountId || sendMut.isPending}
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
        <button className="p-1.5 rounded hover:bg-surface-2 text-text-tertiary" title={t('mail_attach_file')}><Paperclip size={15} /></button>
        <button className="p-1.5 rounded hover:bg-surface-2 text-text-tertiary" title={t('mail_insert_link')}><Link size={15} /></button>
        <button className="p-1.5 rounded hover:bg-surface-2 text-text-tertiary" title={t('mail_insert_emoji')}><Smile size={15} /></button>
        <button className="p-1.5 rounded hover:bg-surface-2 text-text-tertiary" title={t('mail_insert_image')}><Image size={15} /></button>
        <div className="flex-1" />
        <button
          onClick={onCancel}
          className="p-1.5 rounded hover:bg-danger/10 hover:text-danger text-text-tertiary transition-colors"
          title={t('discard')}
        >
          <Trash2 size={15} />
        </button>
      </div>
    </div>
  )
}

// ── Avatar color ─────────────────────────────────────────────────────────────

const AVATAR_COLORS = [
  '#1a73e8','#d93025','#188038','#e37400','#8430ce',
  '#007b83','#e52592','#185abc','#137333','#c5221f',
]
function avatarColor(name: string): string {
  let h = 0
  for (let i = 0; i < name.length; i++) h = (Math.imul(h, 31) + name.charCodeAt(i)) | 0
  return AVATAR_COLORS[Math.abs(h) % AVATAR_COLORS.length]
}

// Vrai si l'utilisateur est en train de saisir (ne pas intercepter les raccourcis).
function isTyping(): boolean {
  const el = document.activeElement as HTMLElement | null
  return !!el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable)
}
function plainKey(e: KeyboardEvent): boolean {
  return !e.metaKey && !e.ctrlKey && !e.altKey && !isTyping()
}

// ── Tabs ──────────────────────────────────────────────────────────────────────

const TABS = [
  { id: 'principale',    labelKey: 'mail_tab_primary' },
  { id: 'purchases',     labelKey: 'mail_tab_purchases' },
  { id: 'social',        labelKey: 'mail_tab_social' },
  { id: 'notifications', labelKey: 'mail_tab_notifications' },
  { id: 'forums',        labelKey: 'mail_tab_forums' },
  { id: 'promotions',    labelKey: 'mail_tab_promotions' },
] as const
export type MailCategory = typeof TABS[number]['id']

function isPromo(email: string) {
  return /no.?reply|newsletter|noreply|promo|marketing|info@|hello@|contact@|deals?@|offers?@/i.test(email)
}
function isSocial(email: string) {
  return /twitter|facebook|linkedin|instagram|tiktok|youtube|pinterest|snapchat|meta\.com|x\.com/i.test(email)
}
function isNotif(email: string) {
  return /notification|alert|update|security|account|billing|no-?reply.*(notif)/i.test(email)
}
function isPurchase(email: string, subject: string) {
  return /order|commande|re[çc]u|facture|invoice|shipping|livraison|delivery|tracking|colis|exp[ée]di|amazon|paypal|stripe|achat|purchase|payment|paiement|receipt/i.test(`${email} ${subject}`)
}
function isForum(email: string) {
  return /forum|digest|groups?@|discourse|mailing.?list|listserv|community|googlegroups/i.test(email)
}
function threadTab(t: Thread): MailCategory {
  const e = t.last_sender_email ?? ''
  const s = t.subject ?? ''
  if (isSocial(e))         return 'social'
  if (isForum(e))          return 'forums'
  if (isPurchase(e, s))    return 'purchases'
  if (isNotif(e))          return 'notifications'
  if (isPromo(e))          return 'promotions'
  return 'principale'
}

// ── Thread list ───────────────────────────────────────────────────────────────

function ThreadList() {
  const { t } = useTranslation('mail')
  const { currentFolder, currentLabelId, inboxCategory, setInboxCategory, selectedAccount, selectedThread, setSelectedThread, searchQuery, accounts, splitMode, setSplitMode, density, setDensity } = useMailStore()
  const [splitMenuPos, setSplitMenuPos] = useState<MenuDropdownPos | null>(null)
  const qc = useQueryClient()
  const [syncing,          setSyncing]          = useState(false)
  const [checkedIds,       setCheckedIds]       = useState<Set<string>>(new Set())
  const [allChecked,       setAllChecked]       = useState(false)
  const [highlightedId,    setHighlightedId]    = useState<string | null>(null)

  const isStarred   = currentFolder === 'starred'
  const isLabel     = currentFolder === 'label'
  const isImportant = currentFolder === 'important'
  const isSnoozed   = currentFolder === 'snoozed'
  const special     = isStarred || isLabel || isImportant || isSnoozed
  const folder      = isStarred ? 'inbox' : currentFolder
  const isInbox     = currentFolder === 'inbox'
  // Les catégories (onglets) ne filtrent QUE la boîte de réception.
  const activeTab = inboxCategory

  // ── Pagination par curseur, façon Gmail (« 1–50 sur N », ‹ › ) ───────────────
  const PAGE = 50
  const [pageIdx, setPageIdx] = useState(0)
  const [cursors, setCursors] = useState<(string | undefined)[]>([undefined]) // `before` par page
  // Réinitialiser à la 1ʳᵉ page quand le contexte change.
  useEffect(() => { setPageIdx(0); setCursors([undefined]) },
    [currentFolder, currentLabelId, searchQuery, inboxCategory, selectedAccount])
  const before = cursors[pageIdx]

  const { data, isLoading, isFetching } = useQuery({
    queryKey: ['mail-threads', folder, currentLabelId, selectedAccount, isStarred, isImportant, isSnoozed, searchQuery, before],
    queryFn:  () => mailApi.listThreads({
      folder:     special ? undefined : folder,
      label_id:   isLabel ? (currentLabelId ?? undefined) : undefined,
      account_id: selectedAccount ?? undefined,
      starred:    isStarred || undefined,
      important:  isImportant || undefined,
      snoozed:    isSnoozed || undefined,
      search:     searchQuery || undefined,
      before:     before ?? undefined,
      limit:      PAGE,
    }),
    refetchInterval: 60_000,
  })

  const { data: pgCounts } = useQuery({ queryKey: ['mail-counts'], queryFn: mailApi.getCounts })
  const hasMore = data?.has_more ?? false
  const goNext = () => {
    if (!hasMore || !data?.cursor) return
    setCursors(c => { const n = [...c]; n[pageIdx + 1] = data.cursor!; return n })
    setPageIdx(i => i + 1)
  }
  const goPrev = () => { if (pageIdx > 0) setPageIdx(i => i - 1) }

  const allThreads = data?.threads ?? []
  const total = (() => {
    if (!pgCounts || searchQuery) return null
    if (isStarred)               return pgCounts.starred
    if (isImportant)             return pgCounts.important
    if (isSnoozed)               return pgCounts.snoozed
    if (isLabel && currentLabelId) return pgCounts.labels[currentLabelId] ?? 0
    if (folder === 'all')        return Object.values(pgCounts.total).reduce((a, b) => a + b, 0)
    return pgCounts.total[folder] ?? 0
  })()
  const rangeStart = allThreads.length ? pageIdx * PAGE + 1 : 0
  const rangeEnd   = pageIdx * PAGE + allThreads.length

  const threads = useMemo(() =>
    isInbox && !searchQuery ? allThreads.filter(t => threadTab(t) === activeTab) : allThreads,
    [allThreads, activeTab, isInbox, searchQuery]
  )

  // Précharger tous les threads visibles en mémoire
  useEffect(() => {
    for (const t of threads) {
      qc.prefetchQuery({
        queryKey: ['mail-thread', t.id],
        queryFn:  () => mailApi.getThread(t.id),
        staleTime: 2 * 60_000,
      })
    }
  }, [threads, qc])

  async function handleSync() {
    if (syncing) return
    setSyncing(true)
    try {
      const ids = selectedAccount ? [selectedAccount] : accounts.map(a => a.id)
      await Promise.all(ids.map(id => mailApi.triggerSync(id)))
      qc.invalidateQueries({ queryKey: ['mail-threads'] })
      await new Promise(r => setTimeout(r, 5000))
      qc.invalidateQueries({ queryKey: ['mail-threads'] })
    } finally { setSyncing(false) }
  }

  function toggleAll() {
    if (allChecked) { setCheckedIds(new Set()); setAllChecked(false) }
    else { setCheckedIds(new Set(threads.map(t => t.id))); setAllChecked(true) }
  }
  function toggleOne(id: string) {
    setCheckedIds(prev => {
      const next = new Set(prev)
      next.has(id) ? next.delete(id) : next.add(id)
      setAllChecked(next.size === threads.length && threads.length > 0)
      return next
    })
  }
  function selectBy(pred: (t: Thread) => boolean) {
    const next = new Set(threads.filter(pred).map(t => t.id))
    setCheckedIds(next); setAllChecked(next.size === threads.length && threads.length > 0); setSelMenuOpen(false)
  }

  const [selMenuOpen,    setSelMenuOpen]    = useState(false)
  const [bulkSnoozeOpen, setBulkSnoozeOpen] = useState(false)

  const refreshLists = () => {
    qc.invalidateQueries({ queryKey: ['mail-threads'] })
    qc.invalidateQueries({ queryKey: ['mail-counts'] })
  }
  const clearSel = () => { setCheckedIds(new Set()); setAllChecked(false) }
  const selIds = () => [...checkedIds]
  // Actions groupées + unitaires (réutilisées par le survol des lignes).
  const doArchive   = async (ids: string[]) => { await Promise.all(ids.map(id => mailApi.moveThread(id, 'archive'))); clearSel(); refreshLists() }
  const doDelete    = async (ids: string[]) => { await Promise.all(ids.map(id => mailApi.deleteThread(id))); clearSel(); refreshLists() }
  const doRead      = async (ids: string[], r: boolean) => { await Promise.all(ids.map(id => mailApi.readThread(id, r))); clearSel(); refreshLists() }
  const doImportant = async (ids: string[]) => { await Promise.all(ids.map(id => mailApi.importantThread(id))); clearSel(); refreshLists() }
  const doSnooze    = async (ids: string[], until: string) => { await Promise.all(ids.map(id => mailApi.snoozeThread(id, until))); clearSel(); setBulkSnoozeOpen(false); refreshLists() }
  const snoozePresets = () => {
    const now = new Date()
    const later = new Date(now); later.setHours(now.getHours() + 3, 0, 0, 0)
    const tom   = new Date(now); tom.setDate(now.getDate() + 1); tom.setHours(8, 0, 0, 0)
    const week  = new Date(now); week.setDate(now.getDate() + 7); week.setHours(8, 0, 0, 0)
    return [
      { label: t('snooze_later',    { defaultValue: 'Plus tard (3 h)' }),      until: later.toISOString() },
      { label: t('snooze_tomorrow', { defaultValue: 'Demain' }),               until: tom.toISOString() },
      { label: t('snooze_nextweek', { defaultValue: 'La semaine prochaine' }), until: week.toISOString() },
    ]
  }
  const TBtn = ({ onClick, title, children }: { onClick: () => void; title: string; children: React.ReactNode }) => (
    <button onClick={onClick} title={title}
      className="p-1.5 rounded hover:bg-surface-2 text-text-secondary transition-colors">{children}</button>
  )

  // Raccourcis de la liste (actifs sans conversation ouverte) : j/k naviguer,
  // Entrée/o ouvrir, e archiver, #/Retour arrière supprimer, s suivre, x sélectionner.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!plainKey(e) || selectedThread) return
      const idx = threads.findIndex(tt => tt.id === highlightedId)
      const hid = highlightedId
      if (e.key === 'j' || e.key === 'ArrowDown') {
        const n = threads[Math.min((idx < 0 ? -1 : idx) + 1, threads.length - 1)]; if (n) { setHighlightedId(n.id); e.preventDefault() }
      } else if (e.key === 'k' || e.key === 'ArrowUp') {
        const n = threads[Math.max((idx < 0 ? 1 : idx) - 1, 0)]; if (n) { setHighlightedId(n.id); e.preventDefault() }
      } else if ((e.key === 'Enter' || e.key === 'o') && hid) { e.preventDefault(); setSelectedThread(hid) }
      else if (e.key === 'x' && hid) { e.preventDefault(); toggleOne(hid) }
      else if (e.key === 's' && hid) { e.preventDefault(); mailApi.starThread(hid).then(refreshLists).catch(() => {}) }
      else if (e.key === 'e' && hid) { e.preventDefault(); doArchive([hid]) }
      else if ((e.key === '#' || e.key === 'Backspace') && hid) { e.preventDefault(); doDelete([hid]) }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [threads, highlightedId, selectedThread]) // eslint-disable-line react-hooks/exhaustive-deps

  return (
    <div className="flex flex-col bg-white overflow-hidden flex-1 min-w-0">

      {/* ── Toolbar ─────────────────────────────────────────────────────── */}
      {/* flex-wrap : sur mobile la barre passe à la ligne au lieu de déborder
          (pas d'overflow → les menus déroulants `absolute` restent visibles) ;
          desktop inchangé (tout tient sur une ligne). */}
      <div className="flex items-center flex-wrap gap-1 px-4 py-2 border-b border-[#f0f0f0] min-h-[44px] no-print">
        {/* Case + menu de sélection (Tout / Aucun / Lus / Non lus / Suivis) */}
        <div className="relative flex items-center">
          <input
            type="checkbox"
            checked={allChecked}
            onChange={toggleAll}
            className="w-4 h-4 rounded cursor-pointer accent-primary flex-shrink-0"
            title={t('mail_select_all')}
          />
          <button onClick={() => setSelMenuOpen(v => !v)} className="px-0.5 text-text-tertiary hover:text-text-primary">
            <ChevronDown size={14} />
          </button>
          {selMenuOpen && (
            <>
              <div className="fixed inset-0 z-40" onClick={() => setSelMenuOpen(false)} />
              <div className="absolute left-0 top-full mt-1 z-50 bg-white border border-border rounded-lg shadow-lg py-1 w-44 text-sm">
                <button className="w-full px-3 py-1.5 text-left hover:bg-surface-1" onClick={() => selectBy(() => true)}>{t('sel_all', { defaultValue: 'Tout' })}</button>
                <button className="w-full px-3 py-1.5 text-left hover:bg-surface-1" onClick={() => { clearSel(); setSelMenuOpen(false) }}>{t('sel_none', { defaultValue: 'Aucun' })}</button>
                <button className="w-full px-3 py-1.5 text-left hover:bg-surface-1" onClick={() => selectBy(tt => tt.unread_count === 0)}>{t('sel_read', { defaultValue: 'Lus' })}</button>
                <button className="w-full px-3 py-1.5 text-left hover:bg-surface-1" onClick={() => selectBy(tt => tt.unread_count > 0)}>{t('sel_unread', { defaultValue: 'Non lus' })}</button>
                <button className="w-full px-3 py-1.5 text-left hover:bg-surface-1" onClick={() => selectBy(tt => tt.is_starred)}>{t('sel_starred', { defaultValue: 'Suivis' })}</button>
              </div>
            </>
          )}
        </div>
        <button
          onClick={handleSync}
          disabled={syncing}
          title={t('mail_refresh')}
          className="p-1.5 rounded hover:bg-surface-2 text-text-tertiary disabled:opacity-50 transition-colors"
        >
          <RefreshCw size={15} className={(syncing || isFetching) ? 'animate-spin' : ''} />
        </button>

        {/* Actions groupées (sélection > 0) — façon Gmail */}
        {checkedIds.size > 0 && (
          <>
            <div className="w-px h-5 bg-[#e0e0e0] mx-1" />
            <TBtn onClick={() => doArchive(selIds())} title={t('archive', { defaultValue: 'Archiver' })}><Archive size={16} /></TBtn>
            <TBtn onClick={() => doDelete(selIds())} title={t('delete', { defaultValue: 'Supprimer' })}><Trash2 size={16} /></TBtn>
            <TBtn onClick={() => doRead(selIds(), true)} title={t('mail_mark_read', { defaultValue: 'Marquer comme lu' })}><MailOpen size={16} /></TBtn>
            <TBtn onClick={() => doRead(selIds(), false)} title={t('mail_mark_unread', { defaultValue: 'Marquer comme non lu' })}><MailIcon size={16} /></TBtn>
            <TBtn onClick={() => doImportant(selIds())} title={t('folder_important', { defaultValue: 'Important' })}><Bookmark size={16} /></TBtn>
            <div className="relative">
              <TBtn onClick={() => setBulkSnoozeOpen(v => !v)} title={t('snooze', { defaultValue: 'Différer' })}><Clock size={16} /></TBtn>
              {bulkSnoozeOpen && (
                <>
                  <div className="fixed inset-0 z-40" onClick={() => setBulkSnoozeOpen(false)} />
                  <div className="absolute left-0 top-full mt-1 z-50 bg-white border border-border rounded-lg shadow-lg py-1 w-52 text-sm">
                    {snoozePresets().map(p => (
                      <button key={p.until} onClick={() => doSnooze(selIds(), p.until)}
                        className="w-full flex items-center gap-2 px-3 py-2 text-left hover:bg-surface-1">
                        <Clock size={14} className="text-text-tertiary" /> {p.label}
                      </button>
                    ))}
                  </div>
                </>
              )}
            </div>
          </>
        )}

        <div className="flex-1" />
        {checkedIds.size > 0 && (
          <span className="text-xs text-text-secondary mr-2">{t('mail_selected_count', { count: checkedIds.size })}</span>
        )}
        {/* Pagination façon Gmail : « 1–50 sur N » + ‹ › */}
        <div className="flex items-center gap-0.5 text-xs text-text-secondary flex-shrink-0">
          <span className="mr-1 tabular-nums hidden sm:inline">
            {total != null
              ? t('mail_range_of', { start: rangeStart, end: rangeEnd, total, defaultValue: `${rangeStart}–${rangeEnd} sur ${total}` })
              : `${rangeStart}–${rangeEnd}`}
          </span>
          <button onClick={goPrev} disabled={pageIdx === 0}
            title={t('mail_newer', { defaultValue: 'Plus récents' })}
            className="p-1.5 rounded hover:bg-surface-2 disabled:opacity-30 disabled:hover:bg-transparent transition-colors">
            <ChevronLeft size={16} />
          </button>
          <button onClick={goNext} disabled={!hasMore}
            title={t('mail_older', { defaultValue: 'Plus anciens' })}
            className="p-1.5 rounded hover:bg-surface-2 disabled:opacity-30 disabled:hover:bg-transparent transition-colors">
            <ChevronRight size={16} />
          </button>

          {/* Densité d'affichage (normale / compacte) — desktop only */}
          <button
            onClick={() => setDensity(density === 'compact' ? 'comfortable' : 'compact')}
            title={t('density_toggle', { defaultValue: density === 'compact' ? 'Affichage normal' : 'Affichage compact' })}
            className={`hidden lg:block p-1.5 rounded hover:bg-surface-2 transition-colors ${density === 'compact' ? 'text-primary' : ''}`}>
            <AlignJustify size={16} />
          </button>

          {/* Mode Volet Double — masqué sur mobile (pas de place pour des volets) */}
          <div className="ml-1 hidden lg:block">
            <button onClick={e => { const r = e.currentTarget.getBoundingClientRect(); setSplitMenuPos(p => p ? null : { top: r.bottom + 4, left: r.right - 224 }) }}
              title={t('split_toggle', { defaultValue: 'Mode Volet Double' })}
              className={`flex items-center p-1.5 rounded hover:bg-surface-2 transition-colors ${splitMode !== 'none' ? 'text-primary' : ''}`}>
              {splitMode === 'horizontal' ? <Rows2 size={16} /> : <Columns2 size={16} />}
              <ChevronDown size={12} className="-ml-0.5" />
            </button>
            {splitMenuPos && (
              <MenuDropdown
                pos={{ ...splitMenuPos, minWidth: 224 }}
                onClose={() => setSplitMenuPos(null)}
                items={[
                  { type: 'action', icon: <Square size={15} />,   label: t('split_none', { defaultValue: 'Aucune séparation' }),       checked: splitMode === 'none',       onClick: () => setSplitMode('none') },
                  { type: 'action', icon: <Columns2 size={15} />, label: t('split_vertical', { defaultValue: 'Séparation verticale' }),  checked: splitMode === 'vertical',   onClick: () => setSplitMode('vertical') },
                  { type: 'action', icon: <Rows2 size={15} />,    label: t('split_horizontal', { defaultValue: 'Séparation horizontale' }), checked: splitMode === 'horizontal', onClick: () => setSplitMode('horizontal') },
                ]}
              />
            )}
          </div>
        </div>
      </div>

      {/* ── Tabs (catégories) — uniquement dans la boîte de réception ─────── */}
      <div className={`${isInbox && !searchQuery ? 'flex' : 'hidden'} border-b border-[#e0e0e0] overflow-x-auto flex-shrink-0`}>
        {TABS.map(tab => {
          const count = allThreads.filter(t => threadTab(t) === tab.id && t.unread_count > 0).length
          return (
            <button
              key={tab.id}
              onClick={() => setInboxCategory(tab.id)}
              className={`flex items-center gap-2 px-5 py-3 text-sm whitespace-nowrap border-b-2 transition-colors
                ${activeTab === tab.id
                  ? 'border-primary text-primary font-medium'
                  : 'border-transparent text-text-secondary hover:text-text-primary hover:bg-surface-1'
                }`}
            >
              {t(tab.labelKey)}
              {count > 0 && (
                <span className={`text-[11px] font-medium px-1.5 py-0.5 rounded-full leading-none
                  ${tab.id === 'promotions'  ? 'bg-orange-100 text-orange-700' :
                    tab.id === 'social'      ? 'bg-blue-100 text-blue-700' :
                    'bg-surface-2 text-text-secondary'}`}>
                  {t('mail_new_count', { count })}
                </span>
              )}
            </button>
          )
        })}
      </div>

      {/* ── List ─────────────────────────────────────────────────────────── */}
      <div className="flex-1 overflow-y-auto">
        {isLoading ? (
          <div className="flex justify-center py-12">
            <Loader2 size={22} className="animate-spin text-text-tertiary" />
          </div>
        ) : threads.length === 0 ? (
          <div className="flex flex-col items-center justify-center py-16 text-text-tertiary">
            <MailIcon size={36} className="mb-3 opacity-30" />
            <p className="text-sm">{t('mail_no_messages')}</p>
          </div>
        ) : (
          threads.map(thread => (
            <ThreadItem
              key={thread.id}
              thread={thread}
              highlighted={highlightedId === thread.id}
              opened={selectedThread === thread.id}
              checked={checkedIds.has(thread.id)}
              onSingleClick={() => { setHighlightedId(thread.id); if (splitMode !== 'none') setSelectedThread(thread.id) }}
              onDoubleClick={() => { setHighlightedId(thread.id); setSelectedThread(thread.id) }}
              onCheck={e => { e.stopPropagation(); toggleOne(thread.id) }}
              onArchive={() => doArchive([thread.id])}
              onDelete={() => doDelete([thread.id])}
              onMarkRead={() => doRead([thread.id], thread.unread_count > 0)}
              onSnooze={() => doSnooze([thread.id], snoozePresets()[1].until)}
            />
          ))
        )}
      </div>
    </div>
  )
}

function ThreadItem({
  thread, highlighted, opened, checked, onSingleClick, onDoubleClick, onCheck,
  onArchive, onDelete, onMarkRead, onSnooze,
}: {
  thread:         Thread
  highlighted:    boolean
  opened:         boolean
  checked:        boolean
  onSingleClick:  () => void
  onDoubleClick:  () => void
  onCheck:        (e: React.MouseEvent) => void
  onArchive:      () => void
  onDelete:       () => void
  onMarkRead:     () => void
  onSnooze:       () => void
}) {
  const { t, i18n } = useTranslation('mail')
  const [hovered, setHovered] = useState(false)
  const qc = useQueryClient()
  const density = useMailStore(s => s.density)
  const clickTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const unread = thread.unread_count > 0
  const senderDisplay = thread.last_sender_name || thread.last_sender_email || '?'
  const initial = senderDisplay[0]?.toUpperCase() ?? '?'
  const color = avatarColor(senderDisplay)

  const starMut = useMutation({
    mutationFn: () => mailApi.starThread(thread.id),
    onSuccess:  () => qc.invalidateQueries({ queryKey: ['mail-threads'] }),
  })

  // Debounce : le clic simple n'agit que si aucun double-clic ne suit dans 220ms
  function handleClick() {
    // Touch UIs have no double-click: a single tap opens the thread directly
    // (otherwise, with no split panel, a tap would only highlight it).
    if (isCoarsePointer()) { onDoubleClick(); return }
    if (clickTimer.current) clearTimeout(clickTimer.current)
    clickTimer.current = setTimeout(() => {
      clickTimer.current = null
      onSingleClick()
    }, 220)
  }
  function handleDoubleClick() {
    if (clickTimer.current) { clearTimeout(clickTimer.current); clickTimer.current = null }
    onDoubleClick()
  }

  const swipe = useSwipeActions({ onRight: onArchive, onLeft: onDelete })

  return (
    <div
      role="button"
      tabIndex={0}
      onClick={handleClick}
      onDoubleClick={handleDoubleClick}
      onKeyDown={e => { if (e.key === 'Enter') handleDoubleClick() }}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      {...swipe.handlers}
      style={swipe.dx !== 0 ? { transform: `translateX(${swipe.dx}px)`, transition: swipe.swiping ? 'none' : 'transform 0.2s ease', touchAction: 'pan-y' } : { touchAction: 'pan-y' }}
      className={`relative flex items-center ${density === 'compact' ? 'h-[36px]' : 'h-[52px]'} px-3 gap-2 cursor-pointer select-none
        border-b border-[#f0f0f0] group
        ${swipe.dx > 0 ? 'bg-[#1e8e3e]' : swipe.dx < 0 ? 'bg-[#d93025]' :
          opened      ? 'bg-blue-50 shadow-[inset_3px_0_0_#1a73e8]' :
          highlighted ? 'bg-[#e8f0fe]' :
          checked     ? 'bg-yellow-50' :
          hovered     ? 'shadow-[0_1px_3px_rgba(0,0,0,0.16)] z-10 relative' : 'bg-white'}`}
    >
      {/* Checkbox — visible au hover ou si coché */}
      <div className={`w-5 flex-shrink-0 flex items-center justify-center
        ${hovered || checked ? 'opacity-100' : 'opacity-0'}`}>
        <input
          type="checkbox"
          checked={checked}
          onClick={onCheck}
          onChange={() => {}}
          className="w-4 h-4 rounded cursor-pointer accent-primary"
        />
      </div>

      {/* Étoile */}
      <button
        onClick={e => { e.stopPropagation(); starMut.mutate() }}
        className={`w-5 flex-shrink-0 flex items-center justify-center transition-opacity
          ${thread.is_starred ? 'opacity-100' : hovered ? 'opacity-60' : 'opacity-0'}`}
        title={thread.is_starred ? t('mail_unstar') : t('mail_star')}
      >
        <Star size={15} className={thread.is_starred ? 'fill-yellow-400 text-yellow-400' : 'text-text-tertiary'} />
      </button>

      {/* Avatar */}
      <div
        className="w-8 h-8 rounded-full flex items-center justify-center text-[13px] font-semibold text-white flex-shrink-0"
        style={{ backgroundColor: color }}
      >
        {initial}
      </div>

      {/* Expéditeur — largeur fixe */}
      <div className={`flex-shrink-0 truncate text-sm w-40
        ${unread ? 'font-semibold text-text-primary' : 'font-normal text-text-secondary'}`}>
        {senderDisplay}
      </div>

      {/* Sujet + extrait */}
      <div className="flex-1 min-w-0 flex items-center gap-0 overflow-hidden">
        <span className={`text-sm truncate flex-shrink-0 max-w-[60%]
          ${unread ? 'font-semibold text-text-primary' : 'text-text-primary'}`}>
          {thread.subject || t('mail_no_subject')}
        </span>
        {thread.snippet && (
          <span className="text-sm text-text-tertiary truncate ml-1">
            &nbsp;–&nbsp;{thread.snippet}
          </span>
        )}
        {thread.has_attachments && (
          <Paperclip size={13} className="text-text-tertiary ml-2 flex-shrink-0" />
        )}
      </div>

      {/* Actions au survol (façon Gmail : Archiver / Supprimer / Lu-Non lu / Différer) */}
      {hovered && (
        <div className="flex items-center gap-0.5 flex-shrink-0 ml-2"
             onClick={e => e.stopPropagation()}>
          <button onClick={onArchive} className="p-1.5 rounded hover:bg-surface-2 text-text-tertiary" title={t('archive', { defaultValue: 'Archiver' })}>
            <Archive size={14} />
          </button>
          <button onClick={onDelete} className="p-1.5 rounded hover:bg-surface-2 text-text-tertiary" title={t('delete')}>
            <Trash2 size={14} />
          </button>
          <button onClick={onMarkRead} className="p-1.5 rounded hover:bg-surface-2 text-text-tertiary"
                  title={t(thread.unread_count > 0 ? 'mail_mark_read' : 'mail_mark_unread', { defaultValue: 'Marquer comme lu' })}>
            {thread.unread_count > 0 ? <MailOpen size={14} /> : <MailIcon size={14} />}
          </button>
          <button onClick={onSnooze} className="p-1.5 rounded hover:bg-surface-2 text-text-tertiary" title={t('snooze', { defaultValue: 'Différer' })}>
            <Clock size={14} />
          </button>
        </div>
      )}

      {/* Date */}
      <div className={`text-xs flex-shrink-0 text-right min-w-[64px]
        ${unread ? 'font-semibold text-text-primary' : 'text-text-tertiary'}
        ${hovered ? 'hidden' : ''}`}>
        {formatDate(thread.last_message_at, t, i18n.language)}
      </div>
    </div>
  )
}

// ── Attachment row ─────────────────────────────────────────────────────────────

function AttachmentRow({
  att, index, messageId, onOpenPdf,
}: { att: Attachment; index: number; messageId: string; onOpenPdf: (url: string, name: string) => void }) {
  const { t } = useTranslation('mail')
  const url   = mailApi.attachmentUrl(messageId, index)
  const isPdf = att.mime === 'application/pdf'

  return (
    <div className="flex items-center gap-2 px-3 py-2 rounded-lg border border-border hover:bg-surface-1 transition-colors group">
      <FileText size={16} className="text-text-tertiary flex-shrink-0" />
      <button
        onClick={() => isPdf ? onOpenPdf(url, att.name) : window.open(url, '_blank')}
        className="flex-1 text-left text-sm text-text-primary truncate hover:text-primary transition-colors"
      >
        {att.name}
      </button>
      <span className="text-xs text-text-tertiary flex-shrink-0">
        {att.size > 0
          ? att.size < 1024 * 1024
            ? t('mail_size_kb', { size: Math.round(att.size / 1024) })
            : t('mail_size_mb', { size: (att.size / 1024 / 1024).toFixed(1) })
          : ''}
      </span>
      <a
        href={url}
        download={att.name}
        onClick={e => e.stopPropagation()}
        className="p-1 rounded opacity-0 group-hover:opacity-100 hover:bg-surface-2 transition-all text-text-tertiary"
        title={t('mail_download')}
      >
        <Download size={14} />
      </a>
    </div>
  )
}

// ── Détection langue étrangère (heuristique simple) ──────────────────────────

function isLikelyForeign(text: string | null): boolean {
  if (!text) return false
  const frWords = /\b(le|la|les|de|du|des|un|une|et|est|en|pour|que|qui|sur|avec|par|dans|au|aux|je|tu|il|elle|nous|vous|ils|elles)\b/gi
  const matches = (text.match(frWords) ?? []).length
  const words   = text.trim().split(/\s+/).length
  return words > 30 && matches / words < 0.05
}

// ── Message card (Gmail style) ────────────────────────────────────────────────

function MessageCard({
  message, isLast, onReply, onForward, onDelete, onMarkUnread, onOpenPdf,
}: {
  message:      EmailMessage
  isLast:       boolean
  onReply:      () => void
  onForward:    () => void
  onDelete:     () => void
  onMarkUnread: () => void
  onOpenPdf:    (url: string, name: string) => void
}) {
  const { t, i18n } = useTranslation('mail')
  const qc = useQueryClient()
  const [expanded,      setExpanded]      = useState(isLast)
  const [starred,       setStarred]       = useState(message.is_starred)
  const [showTranslate, setShowTranslate] = useState(true)
  const [actionsAnchor, setActionsAnchor] = useState<DOMRect | null>(null)

  const starMut = useMutation({
    mutationFn: () => mailApi.starMessage(message.id),
    onSuccess:  (data) => setStarred(data.is_starred),
  })

  const blockMut = useMutation({
    mutationFn: () => mailApi.blockSender(message.from_email),
    onSuccess:  () => {
      qc.invalidateQueries({ queryKey: ['mail-blocked'] })
      qc.invalidateQueries({ queryKey: ['mail-threads'] })
      qc.invalidateQueries({ queryKey: ['mail-counts'] })
    },
  })

  const senderDisplay = message.from_name || message.from_email
  const initial = senderDisplay[0]?.toUpperCase() ?? '?'
  const color   = avatarColor(senderDisplay)
  const foreign = isLikelyForeign(message.body_text)

  const formatFullDate = (s: string) => {
    const d = new Date(s)
    return d.toLocaleDateString(i18n.language, { day: 'numeric', month: 'long', year: 'numeric' })
      + ' ' + d.toLocaleTimeString(i18n.language, { hour: '2-digit', minute: '2-digit' })
  }

  if (!expanded) {
    return (
      <button
        onClick={() => setExpanded(true)}
        className="w-full flex items-center gap-3 py-3 hover:bg-[#f1f3f4] rounded-lg text-left"
      >
        <div className="w-9 h-9 rounded-full flex items-center justify-center text-[13px] font-semibold text-white flex-shrink-0"
             style={{ backgroundColor: color }}>
          {initial}
        </div>
        <div className="flex-1 min-w-0">
          <span className="text-sm font-semibold text-[#202124]">{senderDisplay}</span>
          <span className="text-sm text-[#5f6368] ml-2 truncate">{message.body_text?.substring(0, 100)}</span>
        </div>
        <span className="text-xs text-[#5f6368] flex-shrink-0 mr-2">{formatDate(message.received_at, t, i18n.language)}</span>
      </button>
    )
  }

  return (
    <div className="mb-2">
      {/* ── En-tête expéditeur ────────────────────────────────────────────────── */}
      <div className="flex items-start gap-3 py-3">
        {/* Avatar */}
        <div className="w-10 h-10 rounded-full flex items-center justify-center text-[15px] font-semibold
                        text-white flex-shrink-0 mt-0.5 select-none"
             style={{ backgroundColor: color }}>
          {initial}
        </div>

        <div className="flex-1 min-w-0">
          <div className="flex items-start justify-between gap-2">
            {/* Expéditeur */}
            <div className="min-w-0 flex-1">
              <span className="text-sm font-semibold text-[#202124]">
                {message.from_name || message.from_email}
              </span>
              {message.from_name && (
                <span className="text-xs text-[#5f6368] ml-1.5">
                  &lt;{message.from_email}&gt;
                </span>
              )}
              <span className="text-xs text-[#5f6368] mx-1.5">·</span>
              <button className="text-xs text-[#1a73e8] hover:underline">
                {t('mail_unsubscribe')}
              </button>
            </div>
            {/* Date + actions */}
            <div className="flex items-center gap-0.5 flex-shrink-0" onClick={e => e.stopPropagation()}>
              <span className="text-xs text-[#5f6368] mr-2 whitespace-nowrap">
                {formatFullDate(message.received_at)}
              </span>
              <button
                onClick={() => starMut.mutate()}
                className="p-1.5 rounded-full hover:bg-[#f1f3f4] transition-colors"
                title={t('mail_star')}
              >
                <Star size={16} className={starred ? 'fill-yellow-400 text-yellow-400' : 'text-[#5f6368]'} />
              </button>
              <button
                onClick={onReply}
                className="p-1.5 rounded-full hover:bg-[#f1f3f4] text-[#5f6368]"
                title={t('mail_reply')}
              >
                <Reply size={16} />
              </button>
              <button
                onClick={e => setActionsAnchor(r => r ? null : e.currentTarget.getBoundingClientRect())}
                className="p-1.5 rounded-full hover:bg-[#f1f3f4] text-[#5f6368]"
                title={t('more_options')}
              >
                <MoreVertical size={16} />
              </button>
            </div>
          </div>
          {/* Destinataires */}
          <div className="mt-0.5 text-xs text-[#5f6368]">
            {t('mail_to_prefix')} {(message.to_addresses as Array<{name?:string;email:string}>).map(a => a.name || a.email).join(', ')}
            {(message.cc_addresses as Array<{name?:string;email:string}>).length > 0 && (
              <span> · {t('mail_cc_prefix')} {(message.cc_addresses as Array<{name?:string;email:string}>).map(a => a.name || a.email).join(', ')}</span>
            )}
          </div>
        </div>
      </div>

      {/* ── Bandeau traduction ───────────────────────────────────────────────── */}
      {foreign && showTranslate && (
        <div className="mx-2 mb-2 flex items-center gap-2 px-3 py-2 bg-amber-50 border border-amber-200 rounded-lg text-sm">
          <Languages size={15} className="text-amber-600 flex-shrink-0" />
          <span className="text-text-secondary flex-1">{t('mail_foreign_banner')}</span>
          <button className="text-primary hover:underline text-xs whitespace-nowrap">{t('mail_translate_message')}</button>
          <button onClick={() => setShowTranslate(false)} className="p-0.5 rounded hover:bg-amber-100 text-text-tertiary">
            <X size={14} />
          </button>
        </div>
      )}

      {/* ── Corps ─────────────────────────────────────────────────────────────── */}
      <div className="mt-1">
        {message.body_html ? (
          <EmailHtmlView html={message.body_html} />
        ) : (
          <pre className="text-sm text-[#202124] whitespace-pre-wrap font-sans leading-relaxed py-3">
            {message.body_text}
          </pre>
        )}
      </div>

      {/* ── Pièces jointes ─────────────────────────────────────────────────────── */}
      {message.attachments.length > 0 && (
        <div className="mt-3 mb-2 space-y-1.5">
          <p className="text-xs font-medium text-[#5f6368] mb-2 flex items-center gap-1.5">
            <Paperclip size={13} />
            {t('mail_attachment_count', { count: message.attachments.length })}
          </p>
          {(message.attachments as Attachment[]).map((att, i) => (
            <AttachmentRow key={i} att={att} index={i} messageId={message.id} onOpenPdf={onOpenPdf} />
          ))}
        </div>
      )}

      {/* ── Répondre / Transférer ─────────────────────────────────────────────── */}
      {isLast && (
        <div className="flex items-center gap-3 pt-5 pb-2 border-t border-[#e0e0e0] mt-4">
          <Button
            variant="secondary"
            onClick={onReply}
            icon={<Reply size={15} className="text-[#5f6368]" />}
          >
            {t('mail_reply')}
          </Button>
          <Button
            variant="secondary"
            onClick={onForward}
            icon={<Forward size={15} className="text-[#5f6368]" />}
          >
            {t('mail_forward')}
          </Button>
          <button
            className="p-2 rounded-full border border-[#dadce0] hover:bg-[#f1f3f4] text-[#5f6368]"
            title={t('mail_react')}
          >
            <Smile size={15} />
          </button>
        </div>
      )}

      {/* ── Menu contextuel ──────────────────────────────────────────────────── */}
      {actionsAnchor && (
        <MessageActionsMenu
          anchorRect={actionsAnchor}
          onClose={() => setActionsAnchor(null)}
          onReply={onReply}
          onForward={onForward}
          onDelete={onDelete}
          onMarkUnread={onMarkUnread}
          onBlock={() => blockMut.mutate()}
        />
      )}
    </div>
  )
}

// ── Thread reader ─────────────────────────────────────────────────────────────

function ThreadReader({ onOpenPdf }: { onOpenPdf: (url: string, name: string) => void }) {
  const { t } = useTranslation('mail')
  const { selectedThread, setSelectedThread, currentFolder } = useMailStore()
  const qc = useQueryClient()
  const [inlineMode, setInlineMode] = useState<'reply' | 'forward' | null>(null)
  const [inlineMsg,  setInlineMsg]  = useState<EmailMessage | null>(null)

  useEffect(() => { setInlineMode(null); setInlineMsg(null) }, [selectedThread])

  const { data, isLoading } = useQuery({
    queryKey: ['mail-thread', selectedThread],
    queryFn:  () => mailApi.getThread(selectedThread!),
    enabled:  !!selectedThread,
  })

  const deleteMut = useMutation({
    mutationFn: (id: string) => mailApi.deleteThread(id),
    onSuccess: () => {
      setSelectedThread(null)
      qc.invalidateQueries({ queryKey: ['mail-threads'] })
    },
  })

  const deleteMessageMut = useMutation({
    mutationFn: (id: string) => mailApi.deleteMessage(id),
    onSuccess:  () => qc.invalidateQueries({ queryKey: ['mail-thread', selectedThread] }),
  })

  const markUnreadMut = useMutation({
    mutationFn: (id: string) => mailApi.markRead(id, false),
    onSuccess:  () => {
      qc.invalidateQueries({ queryKey: ['mail-thread', selectedThread] })
      qc.invalidateQueries({ queryKey: ['mail-threads'] })
    },
  })

  const importantMut = useMutation({
    mutationFn: (id: string) => mailApi.importantThread(id),
    onSuccess:  () => {
      qc.invalidateQueries({ queryKey: ['mail-threads'] })
      qc.invalidateQueries({ queryKey: ['mail-counts'] })
    },
  })

  const snoozeMut = useMutation({
    mutationFn: ({ id, until }: { id: string; until: string | null }) => mailApi.snoozeThread(id, until),
    onSuccess:  () => {
      setSnoozeOpen(false)
      setSelectedThread(null)
      qc.invalidateQueries({ queryKey: ['mail-threads'] })
      qc.invalidateQueries({ queryKey: ['mail-counts'] })
    },
  })
  const [snoozeOpen, setSnoozeOpen] = useState(false)

  // Marquer comme spam (folder='spam') ou « pas spam » (folder='inbox') :
  // déplace le fil ET entraîne le classifieur bayésien côté backend.
  const spamMut = useMutation({
    mutationFn: ({ id, folder }: { id: string; folder: 'spam' | 'inbox' }) => mailApi.moveThread(id, folder),
    onSuccess:  () => {
      setSelectedThread(null)
      qc.invalidateQueries({ queryKey: ['mail-threads'] })
      qc.invalidateQueries({ queryKey: ['mail-counts'] })
    },
  })

  const muteMut = useMutation({
    mutationFn: (id: string) => mailApi.muteThread(id),
    onSuccess:  () => {
      setSelectedThread(null)
      qc.invalidateQueries({ queryKey: ['mail-threads'] })
    },
  })

  const snoozePresets = () => {
    const now = new Date()
    const later = new Date(now);    later.setHours(now.getHours() + 3, 0, 0, 0)
    const tom = new Date(now);      tom.setDate(now.getDate() + 1); tom.setHours(8, 0, 0, 0)
    const week = new Date(now);     week.setDate(now.getDate() + 7); week.setHours(8, 0, 0, 0)
    return [
      { label: t('snooze_later',    { defaultValue: 'Plus tard (3 h)' }),       until: later.toISOString() },
      { label: t('snooze_tomorrow', { defaultValue: 'Demain' }),                until: tom.toISOString() },
      { label: t('snooze_nextweek', { defaultValue: 'La semaine prochaine' }),  until: week.toISOString() },
    ]
  }

  // Raccourcis du lecteur : Échap/u=retour, e=archiver, #/Retour arrière=supprimer, r=répondre, f=transférer.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!plainKey(e) || !selectedThread || !data) return
      const { thread, messages } = data
      switch (e.key) {
        case 'Escape': case 'u': e.preventDefault(); setSelectedThread(null); break
        case 'e': e.preventDefault(); mailApi.moveThread(thread.id, 'archive').then(() => {
          setSelectedThread(null); qc.invalidateQueries({ queryKey: ['mail-threads'] }); qc.invalidateQueries({ queryKey: ['mail-counts'] })
        }); break
        case '#': case 'Backspace': e.preventDefault(); deleteMut.mutate(thread.id); break
        case 'r': { e.preventDefault(); const m = messages[messages.length - 1]; if (m) { setInlineMode('reply'); setInlineMsg(m) } break }
        case 'f': { e.preventDefault(); const m = messages[messages.length - 1]; if (m) { setInlineMode('forward'); setInlineMsg(m) } break }
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [selectedThread, data, deleteMut, qc, setSelectedThread])

  if (isLoading) {
    return (
      <div className="flex-1 flex items-center justify-center bg-white">
        <Loader2 size={24} className="animate-spin text-text-tertiary" />
      </div>
    )
  }

  const { thread, messages } = data!

  const TBtn = ({ onClick, title, children, danger }: {
    onClick?: () => void; title: string; children: React.ReactNode; danger?: boolean
  }) => (
    <button
      onClick={onClick}
      title={title}
      className={`p-2 rounded-full transition-colors
        ${danger
          ? 'hover:bg-danger/10 hover:text-danger text-text-secondary'
          : 'hover:bg-[#f1f3f4] text-[#444746]'}`}
    >
      {children}
    </button>
  )

  return (
    <div className="flex-1 flex flex-col min-w-0 overflow-hidden bg-white">

      {/* ══ Toolbar ══════════════════════════════════════════════════════════ */}
      {/* flex-wrap + min-h : la barre d'actions (nombreux boutons) passe à la ligne
          sur mobile sans déborder ni clipper les menus ; desktop = une seule ligne. */}
      <div className="flex items-center flex-wrap gap-0.5 px-2 min-h-[48px] py-1 border-b border-[#e0e0e0] flex-shrink-0 no-print">

        {/* Retour à la liste */}
        <TBtn onClick={() => setSelectedThread(null)} title={t('back')}>
          <ChevronLeft size={20} />
        </TBtn>

        <div className="w-px h-5 bg-[#e0e0e0] mx-1" />

        {/* Actions */}
        <TBtn title={t('archive')}><Archive size={18} /></TBtn>
        {currentFolder === 'spam'
          ? <TBtn title={t('not_spam', { defaultValue: 'Pas un spam' })} onClick={() => spamMut.mutate({ id: thread.id, folder: 'inbox' })}>
              <ShieldCheck size={18} />
            </TBtn>
          : <TBtn title={t('spam_report')} onClick={() => spamMut.mutate({ id: thread.id, folder: 'spam' })}>
              <ShieldAlert size={18} />
            </TBtn>}
        <TBtn title={t('delete')} danger onClick={() => deleteMut.mutate(thread.id)}>
          <Trash2 size={18} />
        </TBtn>
        <TBtn title={t('folder_important', { defaultValue: 'Important' })} onClick={() => importantMut.mutate(thread.id)}>
          <Bookmark size={18} className={thread.is_important ? 'fill-amber-400 text-amber-500' : ''} />
        </TBtn>
        <TBtn title={t('mute', { defaultValue: 'Ignorer la conversation' })} onClick={() => muteMut.mutate(thread.id)}>
          <BellOff size={18} />
        </TBtn>
        <div className="relative">
          <TBtn title={t('snooze', { defaultValue: 'Différer' })} onClick={() => setSnoozeOpen(v => !v)}>
            <Clock size={18} />
          </TBtn>
          {snoozeOpen && (
            <>
              <div className="fixed inset-0 z-40" onClick={() => setSnoozeOpen(false)} />
              <div className="absolute left-0 top-full mt-1 z-50 bg-white border border-border rounded-lg shadow-lg py-1 w-52">
                {snoozePresets().map(p => (
                  <button key={p.until} onClick={() => snoozeMut.mutate({ id: thread.id, until: p.until })}
                    className="w-full flex items-center gap-2 px-3 py-2 text-sm text-text-primary hover:bg-surface-1 text-left">
                    <Clock size={14} className="text-text-tertiary" /> {p.label}
                  </button>
                ))}
              </div>
            </>
          )}
        </div>
        <TBtn title={t('move_to')}><Download size={18} /></TBtn>
        <TBtn title={t('more_options')}><MoreVertical size={18} /></TBtn>

        <div className="flex-1" />

        {/* Navigation */}
        <span className="text-xs text-[#444746] mr-1 select-none">
          {t('mail_message_count', { count: messages.length })}
        </span>
        <TBtn title={t('mail_older')}><ChevronLeft size={18} /></TBtn>
        <TBtn title={t('mail_newer')}><ChevronRight size={18} /></TBtn>

        <div className="w-px h-5 bg-[#e0e0e0] mx-1" />
        <TBtn title={t('mail_display_settings')}><Settings2 size={18} /></TBtn>
        <TBtn title={t('mail_open_new_window')}><ExternalLink size={18} /></TBtn>
      </div>

      {/* ══ Zone de lecture (scrollable) ══════════════════════════════════════ */}
      <div className="flex-1 overflow-y-auto">
        <div className="w-full px-8 py-4">

          {/* ── Sujet + label ─────────────────────────────────────────────── */}
          <div className="flex items-start gap-3 mb-4">
            <h1 className="flex-1 text-[22px] font-normal text-[#202124] leading-snug break-words">
              {thread.subject || t('mail_no_subject')}
            </h1>
            <div className="flex items-center gap-2 flex-shrink-0 mt-1">
              <span className="flex items-center gap-1 text-[11px] text-[#444746] bg-[#f1f3f4]
                               px-2.5 py-1 rounded border border-[#dadce0] whitespace-nowrap">
                {t('folder_inbox')}
                <button className="ml-1 hover:text-[#202124] leading-none">×</button>
              </span>
              <button className="p-1.5 rounded-full hover:bg-[#f1f3f4] text-[#444746]" title={t('print')}>
                <Printer size={16} />
              </button>
              <button className="p-1.5 rounded-full hover:bg-[#f1f3f4] text-[#444746]" title={t('mail_new_window')}>
                <ExternalLink size={16} />
              </button>
            </div>
          </div>

          {/* Avertissement bayésien : message resté en boîte mais jugé douteux. */}
          {currentFolder === 'inbox' && messages.some(m => (m.spam_score ?? 0) >= 0.7) && (
            <div className="mb-3 flex items-center gap-2 px-3 py-2 rounded-lg bg-amber-50 border border-amber-200 text-[13px] text-amber-800">
              <ShieldAlert size={16} className="flex-shrink-0 text-amber-500" />
              <span className="flex-1">{t('spam_suspected', { defaultValue: 'Ce message ressemble à un indésirable.' })}</span>
              <button
                onClick={() => spamMut.mutate({ id: thread.id, folder: 'spam' })}
                className="px-2.5 py-1 rounded-md bg-amber-500 text-white text-xs font-medium hover:bg-amber-600"
              >
                {t('spam_report')}
              </button>
            </div>
          )}

          {/* ── Messages ──────────────────────────────────────────────────── */}
          {messages.map((msg, i) => (
            <MessageCard
              key={msg.id}
              message={msg}
              isLast={i === messages.length - 1}
              onReply={() => { setInlineMode('reply'); setInlineMsg(msg) }}
              onForward={() => { setInlineMode('forward'); setInlineMsg(msg) }}
              onDelete={() => deleteMessageMut.mutate(msg.id)}
              onMarkUnread={() => markUnreadMut.mutate(msg.id)}
              onOpenPdf={onOpenPdf}
            />
          ))}

          {/* ── Zone de réponse inline ──────────────────────────────────── */}
          {inlineMode && inlineMsg && (
            <div className="mt-4 mb-2">
              <InlineCompose
                mode={inlineMode}
                message={inlineMsg}
                onSent={() => setInlineMode(null)}
                onCancel={() => setInlineMode(null)}
              />
            </div>
          )}

          <div className="h-8" />
        </div>
      </div>
    </div>
  )
}

// ── Toast décompte « Annuler l'envoi » ────────────────────────────────────────
// Reprend la mécanique du décompte de suppression de drive (carte + barre de
// progression qui se vide en `duration` ms), réadaptée aux couleurs du mail
// (accent bleu primaire, action positive « envoyé » plutôt que destructive).

function UndoSendToast({ label, undoLabel, onCancel }: {
  label: string; undoLabel: string; onCancel: () => void
}) {
  const duration = useUndoSendStore(s => s.duration)
  // Barre qui se vide de 100 % → 0 % en `duration` ms (transition CSS linéaire).
  const [width, setWidth] = useState(100)
  useEffect(() => {
    const r = requestAnimationFrame(() => setWidth(0))
    return () => cancelAnimationFrame(r)
  }, [])

  return (
    <div className="fixed bottom-6 left-6 z-[100] w-80 rounded-xl border border-primary-light bg-surface-0 shadow-lg overflow-hidden">
      <div className="flex items-center gap-3 px-4 py-3">
        <Send size={18} className="text-primary" />
        <span className="flex-1 text-sm text-text-primary truncate">{label}</span>
        <button
          onClick={onCancel}
          className="flex items-center gap-1 text-sm font-medium text-primary hover:underline shrink-0"
        >
          <Undo2 size={14} /> {undoLabel}
        </button>
      </div>
      <div className="h-1 bg-surface-2">
        <div
          className="h-full bg-primary"
          style={{ width: `${width}%`, transition: `width ${duration}ms linear` }}
        />
      </div>
    </div>
  )
}

// ── Main MailApp ──────────────────────────────────────────────────────────────

export default function MailApp() {
  const { t } = useTranslation('mail')
  const { composeOpen, setComposeOpen, setComposeInitial, setAccounts, accounts, setCurrentFolder, currentFolder, selectedThread, splitMode } = useMailStore()
  const undoPayload = useUndoSendStore(s => s.payload)
  const cancelUndo  = useUndoSendStore(s => s.cancel)

  // Raccourci global : « c » = nouveau message.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!plainKey(e)) return
      if (e.key === 'c') { e.preventDefault(); setComposeOpen(true) }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [setComposeOpen])
  const { pathname } = useLocation()
  const [pdfUrl,  setPdfUrl]  = useState<string | null>(null)
  const [pdfName, setPdfName] = useState('')
  const openPdf = (url: string, name: string) => { setPdfUrl(url); setPdfName(name) }

  const { data: accountsData } = useQuery({
    queryKey: ['mail-accounts'],
    queryFn:  mailApi.listAccounts,
  })

  useEffect(() => {
    const labelMatch = pathname.match(/^\/mail\/label\/([^/]+)/)
    if (labelMatch) setCurrentFolder('label', labelMatch[1])
    else setCurrentFolder(folderFromPath(pathname))
  }, [pathname, setCurrentFolder])

  useEffect(() => {
    if (accountsData?.accounts) setAccounts(accountsData.accounts)
  }, [accountsData, setAccounts])

  const hasAccounts = accounts.length > 0

  if (!hasAccounts && accountsData !== undefined) {
    return (
      <div className="flex flex-col items-center justify-center h-full gap-4">
        <MailIcon size={48} className="text-text-tertiary opacity-40" />
        <div className="text-center">
          <p className="text-text-primary font-medium mb-1">{t('no_account')}</p>
          <p className="text-sm text-text-tertiary mb-4">
            {t('mail_no_account_hint')}
          </p>
          <a
            href="/mail/settings"
            className="inline-flex items-center h-9 px-4 text-sm font-medium rounded-md
                       bg-primary text-white hover:bg-primary-hover transition-colors"
          >
            {t('mail_configure_account')}
          </a>
        </div>
      </div>
    )
  }

  return (
    <div className="flex h-full overflow-hidden">
      {/* ── Navigation dossiers (intégrée, remplace AppSidebar) ───────────
          Masquée sur mobile : elle écraserait la liste des fils. Les dossiers
          restent accessibles via le tiroir du core (hamburger), où
          MailSidebarBody est aussi enregistré (cf. entry.ts). */}
      <div className="hidden lg:flex w-64 flex-shrink-0 flex-col overflow-hidden border-r border-[#e0e0e0] bg-white py-2">
        <MailSidebarBody />
      </div>

      {/* ── Zone principale ───────────────────────────────────────────────── */}
      {currentFolder === 'subscriptions' ? <SubscriptionsView />
        : currentFolder === 'scheduled'  ? <ScheduledView />
        : splitMode === 'none'
          ? (selectedThread ? <ThreadReader onOpenPdf={openPdf} /> : <ThreadList />)
          : (
            <div className={splitMode === 'vertical'
              ? 'flex flex-1 min-w-0 overflow-hidden'
              : 'flex flex-col flex-1 min-h-0 overflow-hidden'}>
              <div className={splitMode === 'vertical'
                ? 'w-[42%] min-w-[340px] max-w-[600px] border-r border-[#e0e0e0] flex flex-col overflow-hidden'
                : 'h-[45%] min-h-[200px] border-b border-[#e0e0e0] flex flex-col overflow-hidden'}>
                <ThreadList />
              </div>
              <div className="flex-1 min-w-0 min-h-0 overflow-hidden flex flex-col">
                {selectedThread
                  ? <ThreadReader onOpenPdf={openPdf} />
                  : (
                    <div className="flex-1 flex items-center justify-center text-text-tertiary text-sm bg-surface-1/30">
                      {t('split_pick', { defaultValue: 'Sélectionnez une conversation à lire' })}
                    </div>
                  )}
              </div>
            </div>
          )
      }

      {composeOpen && <ComposeWindow />}

      {/* Toast décompte « Annuler l'envoi » (même mécanique que le décompte de
          suppression de drive : carte + barre de progression qui se vide). */}
      {undoPayload && (
        <UndoSendToast
          label={t('undo_sent', { defaultValue: 'Message envoyé' })}
          undoLabel={t('undo_cancel', { defaultValue: 'Annuler' })}
          onCancel={() => {
            const p = cancelUndo()
            if (p) {
              setComposeInitial({ to: p.to_addresses, cc: p.cc_addresses ?? [], subject: p.subject, bodyHtml: p.body_html })
              setComposeOpen(true)
            }
          }}
        />
      )}
      {pdfUrl && (
        <PdfViewerModal url={pdfUrl} filename={pdfName} onClose={() => setPdfUrl(null)} />
      )}
    </div>
  )
}
