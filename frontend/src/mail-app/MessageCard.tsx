import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import {
  ChevronDown, Reply, ReplyAll, Forward, Star, MoreVertical, Paperclip, X,
  Lock, ShieldCheck, ShieldAlert,
} from 'lucide-react'
import { useIsMobile } from '@ui'
import { mailApi, EmailMessage, Attachment } from '../api'
import { useMailStore } from '../store'
import { unsubscribeTarget } from '../MailViews'
import { formatDate, formatFullDate, replyAllExtras } from './helpers'
import EmailHtmlView from './EmailHtmlView'
import AttachmentRow from './AttachmentRow'
import MessageActionsMenu from './MessageActionsMenu'
import MessageDetails from './MessageDetails'
import SenderAvatar from './SenderAvatar'
import RichCards from './richCards/RichCards'

/** Reply / Reply-all / Forward: 36px pills with a hairline border, like Gmail's. */
function ReplyPill({ onClick, icon, label }: {
  onClick: () => void; icon: React.ReactNode; label: string
}) {
  return (
    <button
      onClick={onClick}
      className="h-9 px-4 inline-flex items-center gap-2 rounded-full border border-[#dadce0]
                 text-sm text-[#3c4043] bg-white hover:bg-[#f1f3f4] transition-colors"
    >
      <span className="text-[#5f6368]">{icon}</span>
      {label}
    </button>
  )
}

/** One-line preview for a collapsed row: the NEW content only, with the quoted
 *  history stripped (attribution line, "> " quotes, forwarded "De:/From:" header)
 *  — like Gmail, which never shows the quote in the snippet. */
function previewSnippet(text: string | null | undefined): string {
  if (!text) return ''
  const kept: string[] = []
  for (const line of text.split(/\r?\n/)) {
    const t = line.trim()
    if (/^>/.test(t)) break
    if (/\ba\s+écrit\s*:/i.test(t) || /\bwrote:/i.test(t)) break
    if (/^-{2,}\s*(original message|message d'origine|forwarded message|message transféré)/i.test(t)) break
    if (/^(De|From)\s*:\s/i.test(t)) break
    kept.push(line)
  }
  const snippet = kept.join(' ').replace(/\s+/g, ' ').trim()
  // If the whole body was quote (nothing new), fall back to the raw text rather
  // than an empty snippet.
  return snippet || text.replace(/\s+/g, ' ').trim()
}

/** OpenPGP trust chips: "Chiffré" and/or a signature verdict. Shown only when
 *  the message actually carried PGP protection (decrypted/verified server-side),
 *  in the same visual language as the TLS/security row of the details card. */
function PgpBadges({ message }: { message: EmailMessage }) {
  const { t } = useTranslation('mail')
  const encrypted = message.pgp_encrypted === true
  const signed = message.pgp_signature_valid
  if (!encrypted && signed == null) return null

  const chip = 'inline-flex items-center gap-1 h-5 px-2 rounded-full text-[11px] font-medium'
  const fp = message.pgp_signer_fingerprint
  // Group the fingerprint in 4-char blocks for a readable tooltip.
  const fpPretty = fp ? fp.replace(/(.{4})/g, '$1 ').trim() : undefined

  return (
    <div className="mt-1 flex flex-wrap items-center gap-1.5">
      {encrypted && (
        <span
          className={`${chip} bg-[#e6f4ea] text-[#137333]`}
          title={t('mail_pgp_encrypted_hint', { defaultValue: 'Message chiffré de bout en bout (OpenPGP)' })}
        >
          <Lock size={12} />
          {t('mail_pgp_encrypted', { defaultValue: 'Chiffré' })}
        </span>
      )}
      {signed === true && (
        <span
          className={`${chip} bg-[#e6f4ea] text-[#137333]`}
          title={fpPretty
            ? t('mail_pgp_signed_by_fp', { defaultValue: 'Signature vérifiée · {{fp}}', fp: fpPretty })
            : t('mail_pgp_signed_valid_hint', { defaultValue: 'Signature OpenPGP vérifiée' })}
        >
          <ShieldCheck size={12} />
          {t('mail_pgp_signed_valid', { defaultValue: 'Signature vérifiée' })}
        </span>
      )}
      {signed === false && (
        <span
          className={`${chip} bg-[#fce8e6] text-[#c5221f]`}
          title={t('mail_pgp_signed_invalid_hint', { defaultValue: 'Signature OpenPGP absente de vos contacts ou invalide' })}
        >
          <ShieldAlert size={12} />
          {t('mail_pgp_signed_invalid', { defaultValue: 'Signature non vérifiée' })}
        </span>
      )}
    </div>
  )
}

// ── Message card (Gmail style) ────────────────────────────────────────────────

export default function MessageCard({
  message, isLast, collapsible = true, expanded, onToggle, important = false,
  onReply, onReplyAll, onForward, onDelete, onMarkUnread, onOpenPdf, onSpam,
}: {
  message:      EmailMessage
  isLast:       boolean
  /** Open state, owned by the thread so "expand/collapse all" always agrees
   *  with what the reader did by hand. */
  expanded:     boolean
  /** The thread is flagged important — shown in the details card. */
  important?:   boolean
  onToggle:     (open: boolean) => void
  /** False for a thread holding a single message: there is nothing to collapse
   *  into, so its header must not act like a button. */
  collapsible?: boolean
  onReply:      () => void
  onReplyAll:   () => void
  onForward:    () => void
  onDelete:     () => void
  onMarkUnread: () => void
  onOpenPdf:    (url: string, name: string) => void
  onSpam:       () => void
}) {
  const { t, i18n } = useTranslation('mail')
  const qc = useQueryClient()
  const { setSearchQuery, accounts } = useMailStore()
  // Does a "reply all" reach anyone a plain reply wouldn't?
  const hasOtherRecipients =
    replyAllExtras(message, accounts.map(a => a.email_address)).length > 0
  // Mobile: lighter header — short date, no raw address, star + "⋮" only
  // (Reply stays as the big button under the message).
  const isMobile = useIsMobile()
  const [starred,       setStarred]       = useState(message.is_starred)
  const [showOriginal,  setShowOriginal]  = useState(false)
  const [detailsOpen,   setDetailsOpen]   = useState(false)
  const [actionsAnchor, setActionsAnchor] = useState<DOMRect | null>(null)

  // Rebuilds a downloadable .eml from the stored data (headers + HTML body).
  // The raw RFC 5322 message is not kept on the server side.
  const downloadEml = () => {
    const addrs = (v: unknown): string => Array.isArray(v)
      ? v.map(a => (a as { email?: string; name?: string }).name
          ? `"${(a as { name?: string }).name}" <${(a as { email?: string }).email}>`
          : (a as { email?: string }).email ?? '').join(', ')
      : ''
    const headers = [
      `From: ${message.from_name ? `"${message.from_name}" <${message.from_email}>` : message.from_email}`,
      `To: ${addrs(message.to_addresses)}`,
      addrs(message.cc_addresses) ? `Cc: ${addrs(message.cc_addresses)}` : null,
      `Subject: ${message.subject ?? ''}`,
      `Date: ${new Date(message.sent_at ?? message.received_at).toUTCString()}`,
      message.message_id ? `Message-ID: <${message.message_id}>` : null,
      'MIME-Version: 1.0',
      'Content-Type: text/html; charset=utf-8',
    ].filter(Boolean).join('\r\n')
    const blob = new Blob([`${headers}\r\n\r\n${message.body_html ?? message.body_text ?? ''}`], { type: 'message/rfc822' })
    const a = document.createElement('a')
    a.href = URL.createObjectURL(blob)
    a.download = `${(message.subject || 'message').replace(/[/\\:*?"<>|]/g, '_').slice(0, 80)}.eml`
    a.click()
    URL.revokeObjectURL(a.href)
  }

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

  if (!expanded) {
    return (
      // A div, not a button: it holds the star, and a button cannot nest a button.
      <div
        role="button"
        tabIndex={0}
        onClick={() => onToggle(true)}
        onKeyDown={e => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); onToggle(true) } }}
        className="w-full flex items-center gap-3 py-5 hover:bg-[#f1f3f4] text-left cursor-pointer"
      >
        {/* Same avatar metrics as the expanded header — the picture must not
            resize when a message is collapsed or opened. */}
        <SenderAvatar email={message.from_email} name={message.from_name} size={40} dmarc={message.auth_dmarc} />
        {/* Sender and preview on their own line each — one block per line. */}
        <div className="flex-1 min-w-0">
          <div className="text-sm font-semibold text-[#202124] truncate">{senderDisplay}</div>
          <div className="text-sm text-[#5f6368] truncate">{previewSnippet(message.body_text).substring(0, 100)}</div>
        </div>
        <span className="text-xs text-[#5f6368] flex-shrink-0">{formatDate(message.received_at, t, i18n.language)}</span>
        {/* Star stays reachable on a collapsed message, as Gmail keeps it. */}
        <button
          onClick={e => { e.stopPropagation(); starMut.mutate() }}
          className="p-1.5 rounded-full hover:bg-black/[0.08] transition-colors flex-shrink-0"
          title={t('mail_star')}
        >
          <Star size={16} className={starred ? 'fill-yellow-400 text-yellow-400' : 'text-[#5f6368]'} />
        </button>
      </div>
    )
  }

  return (
    <div className="mb-2">
      {/* ── Sender header ─────────────────────────────────────────────────────── */}
      {/* Clicking the header collapses the message again (Gmail behaviour). The
          controls inside it (star, reply, ⋮, recipients chevron) stop the click. */}
      <div
        className={`flex items-start gap-3 py-5 ${collapsible ? 'cursor-pointer' : ''}`}
        onClick={collapsible ? () => onToggle(false) : undefined}
        role={collapsible ? 'button' : undefined}
        aria-expanded={collapsible ? true : undefined}
        title={collapsible ? t('mail_collapse_message', { defaultValue: 'Réduire le message' }) : undefined}
      >
        {/* Avatar */}
        {/* No extra top margin: the avatar must sit at the very same offset as in
            the collapsed row, so it does not shift when the message opens. */}
        <SenderAvatar email={message.from_email} name={message.from_name} size={40} dmarc={message.auth_dmarc} />

        <div className="flex-1 min-w-0">
          <div className="flex items-start justify-between gap-2">
            {/* Sender */}
            <div className="min-w-0 flex-1">
              <span className="text-sm font-semibold text-[#202124]">
                {message.from_name || message.from_email}
              </span>
              {message.from_name && !isMobile && (
                <span className="text-xs text-[#5f6368] ml-1.5">
                  &lt;{message.from_email}&gt;
                </span>
              )}
              {/* "Unsubscribe" — only when the message exposes List-Unsubscribe. */}
              {message.list_unsubscribe && unsubscribeTarget(message.list_unsubscribe) && (
                <>
                  <span className="text-xs text-[#5f6368] mx-1.5">·</span>
                  <button className="text-xs text-[#1a73e8] hover:underline"
                    onClick={e => {
                      e.stopPropagation()
                      const target = unsubscribeTarget(message.list_unsubscribe!)!
                      if (target.startsWith('mailto:')) window.location.href = target
                      else window.open(target, '_blank', 'noopener,noreferrer')
                    }}>
                    {t('mail_unsubscribe')}
                  </button>
                </>
              )}
            </div>
            {/* Date + actions */}
            {/* The date stays part of the collapse target; only the buttons opt out. */}
            <div className="flex items-center gap-0.5 flex-shrink-0">
              <span className="text-xs text-[#5f6368] mr-2 whitespace-nowrap">
                {isMobile ? formatDate(message.received_at, t, i18n.language) : formatFullDate(message.received_at, i18n.language)}
              </span>
              <div className="flex items-center gap-0.5" onClick={e => e.stopPropagation()}>
              <button
                onClick={() => starMut.mutate()}
                className="p-1.5 rounded-full hover:bg-[#f1f3f4] transition-colors"
                title={t('mail_star')}
              >
                <Star size={16} className={starred ? 'fill-yellow-400 text-yellow-400' : 'text-[#5f6368]'} />
              </button>
              {!isMobile && (
                <button
                  onClick={onReply}
                  className="p-1.5 rounded-full hover:bg-[#f1f3f4] text-[#5f6368]"
                  title={t('mail_reply')}
                >
                  <Reply size={16} />
                </button>
              )}
              <button
                // Measure BEFORE updating: React nulls `currentTarget` once the
                // handler returns, so reading it inside the updater throws.
                onClick={e => {
                  const rect = e.currentTarget.getBoundingClientRect()
                  setActionsAnchor(r => r ? null : rect)
                }}
                className="p-1.5 rounded-full hover:bg-[#f1f3f4] text-[#5f6368]"
                title={t('more_options')}
              >
                <MoreVertical size={16} />
              </button>
              </div>
            </div>
          </div>
          {/* Recipients — collapsed, unfolded from the chevron like Gmail does */}
          <div className="mt-0.5 flex items-start gap-1 relative">
            <button
              onClick={e => { e.stopPropagation(); setDetailsOpen(v => !v) }}
              aria-expanded={detailsOpen}
              title={detailsOpen
                ? t('mail_hide_details', { defaultValue: 'Masquer les détails' })
                : t('mail_show_details', { defaultValue: 'Afficher les détails' })}
              className="flex items-center gap-1 text-xs text-[#5f6368] hover:text-text-primary rounded px-1 -ml-1"
            >
              <span className="truncate max-w-[420px]">
                {t('mail_to_prefix')} {(message.to_addresses as Array<{name?:string;email:string}>).map(a => a.name || a.email).join(', ')}
                {(message.cc_addresses as Array<{name?:string;email:string}>).length > 0 && (
                  <> · {t('mail_cc_prefix')} {(message.cc_addresses as Array<{name?:string;email:string}>).map(a => a.name || a.email).join(', ')}</>
                )}
              </span>
              <ChevronDown size={14} className={`flex-shrink-0 transition-transform ${detailsOpen ? 'rotate-180' : ''}`} />
            </button>
          {detailsOpen && (
              <MessageDetails
                important={important}
                message={message}
                lang={i18n.language}
                onClose={() => setDetailsOpen(false)}
              />
            )}
          </div>

          {/* OpenPGP trust chips (encrypted / signature verdict). */}
          <PgpBadges message={message} />

        </div>
      </div>

      {/* ── Message source ("Show original") ────────────────────────────────── */}
      {showOriginal && (
        <div className="mx-2 mb-2 rounded-lg border border-border bg-surface-1 overflow-hidden">
          <div className="flex items-center justify-between px-3 py-1.5 border-b border-border">
            <span className="text-xs font-medium text-text-secondary">{t('mail_show_original')}</span>
            <button onClick={() => setShowOriginal(false)} className="p-0.5 rounded hover:bg-surface-2 text-text-tertiary"><X size={14} /></button>
          </div>
          <pre className="p-3 text-[11px] leading-relaxed text-text-secondary overflow-x-auto max-h-80 whitespace-pre-wrap break-all">
{`Message-ID: ${message.message_id ?? '—'}
From: ${message.from_name ?? ''} <${message.from_email}>
Date: ${new Date(message.sent_at ?? message.received_at).toISOString()}
Folder: ${message.folder}  ·  Spam score: ${message.spam_score ?? '—'}
List-Unsubscribe: ${message.list_unsubscribe ?? '—'}

${message.body_html ?? message.body_text ?? ''}`}
          </pre>
        </div>
      )}

      {/* ── Body ──────────────────────────────────────────────────────────────── */}
      {/* Aligned with the sender name, not the avatar: the avatar sits in a left
          gutter (w-10 + gap-3 = 52px) and the content flows past it, the way
          Gmail lays out a message. Dropped on mobile, where 52px of every line
          is width the screen cannot spare. */}
      {/* Gmail-style rich cards (events/invites, flights, hotels, transit,
          orders…) parsed from the message's schema.org structured data. */}
      <div className={isMobile ? '' : 'ps-[52px]'}>
        <RichCards message={message} />
      </div>

      <div className={`mt-1 ${isMobile ? '' : 'ps-[52px]'}`}>
        {message.body_html ? (
          <EmailHtmlView
            html={message.body_html}
            // Lets the viewer rebuild a De/Envoyé/À/Objet block above the "•••"
            // when the quoted mail has none: its recipient is this message's
            // author, and its subject is the thread's.
            quoteContext={{
              lang:    i18n.language,
              compact: isMobile,
              to:      message.from_name ? `${message.from_name} <${message.from_email}>` : message.from_email,
              subject: message.subject?.replace(/^\s*(re|fwd|tr)\s*:\s*/i, '') || message.subject,
            }}
          />
        ) : (
          <pre className="text-sm text-[#202124] whitespace-pre-wrap font-sans leading-relaxed py-3">
            {message.body_text}
          </pre>
        )}
      </div>

      {/* ── Attachments ────────────────────────────────────────────────────────── */}
      {message.attachments.length > 0 && (
        <div className={`mt-3 mb-2 space-y-1.5 ${isMobile ? '' : 'ps-[52px]'}`}>
          <p className="text-xs font-medium text-[#5f6368] mb-2 flex items-center gap-1.5">
            <Paperclip size={13} />
            {t('mail_attachment_count', { count: message.attachments.length })}
          </p>
          {(message.attachments as Attachment[]).map((att, i) => (
            <AttachmentRow key={i} att={att} index={i} messageId={message.id} onOpenPdf={onOpenPdf} />
          ))}
        </div>
      )}

      {/* ── Reply / Forward ───────────────────────────────────────────────────── */}
      {/* "Reply all" only when it would actually reach someone else: with a
          single correspondent it sends the very same message as "Reply". */}
      {isLast && (
        <div className="flex items-center gap-3 pt-5 pb-2 border-t border-[#e0e0e0] mt-4">
          <ReplyPill onClick={onReply} icon={<Reply size={18} />} label={t('mail_reply')} />
          {hasOtherRecipients && (
            <ReplyPill onClick={onReplyAll} icon={<ReplyAll size={18} />} label={t('mail_reply_all')} />
          )}
          <ReplyPill onClick={onForward} icon={<Forward size={18} />} label={t('mail_forward')} />
        </div>
      )}

      {/* ── Context menu ─────────────────────────────────────────────────────── */}
      {actionsAnchor && (
        <MessageActionsMenu
          anchorRect={actionsAnchor}
          onClose={() => setActionsAnchor(null)}
          onReply={onReply}
          onForward={onForward}
          onDelete={onDelete}
          onMarkUnread={onMarkUnread}
          onBlock={() => blockMut.mutate()}
          onSpam={onSpam}
          onFilterSimilar={() => setSearchQuery(`from:${message.from_email}`)}
          onDownload={downloadEml}
          onShowOriginal={() => setShowOriginal(true)}
        />
      )}
    </div>
  )
}
