import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { X, AlertCircle, HardDrive } from 'lucide-react'
import AttachmentPreview, { type PreviewSource } from './AttachmentPreview'
import type { ComposeAttachment } from './composeAttachments'

const fmtSize = (n: number) =>
  n < 1024 ? `${n} o` : n < 1048576 ? `${Math.round(n / 1024)} Ko` : `${(n / 1048576).toFixed(1)} Mo`

/** Attachments we can preview inline from the local blob (else: download). */
export function isPreviewable(mime: string): boolean {
  return mime.startsWith('image/') || mime === 'application/pdf' || mime.startsWith('text/')
}

// ── Single attachment band (Gmail-style) ─────────────────────────────────────
// A full-width light-gray band. While the file is read/encoded (or an oversized
// file is uploaded to Drive), the name is muted and a bordered progress-bar
// rectangle fills on the right (Gmail state 1/2). Once ready, the name turns
// into a blue link (preview/download, or the public Drive URL for a link) with
// its size, and a × removes it (Gmail state 3). A Drive-link attachment carries
// a Drive glyph and a « lien » badge so the user sees the file became a link.
export function AttachmentChip({ att, onOpen, onRemove }: {
  att:      ComposeAttachment
  onOpen:   (att: ComposeAttachment) => void
  onRemove: (id: string) => void
}) {
  const { t } = useTranslation('mail')
  const reading = att.status === 'reading'
  const ready   = att.status === 'ready'
  const error   = att.status === 'error'
  const isLink  = att.kind === 'link'

  const errorText = error
    ? att.errorKind === 'drive-missing'
      ? t('mail_large_files_drive_missing', { defaultValue: 'Drive indisponible' })
      : t('mail_attach_error', { defaultValue: 'échec' })
    : `(${fmtSize(att.size)})`

  return (
    <div className="group flex items-center gap-3 w-full bg-surface-1 rounded px-3 py-2">
      {/* ── Name + size ─────────────────────────────────────────────────────── */}
      <div className="flex-1 min-w-0 flex items-baseline gap-1.5">
        {isLink && !error && (
          <HardDrive size={14} className="flex-shrink-0 text-primary self-center" />
        )}
        {ready ? (
          // Blue clickable name → preview / download (file) or public Drive URL (link).
          <button
            type="button"
            onClick={() => onOpen(att)}
            title={att.filename}
            className="text-sm text-primary hover:underline truncate text-left"
          >
            {att.filename}
          </button>
        ) : (
          <span className={`flex items-center gap-1.5 text-sm truncate ${error ? 'text-danger' : 'text-text-tertiary'}`}>
            {error && <AlertCircle size={13} className="flex-shrink-0" />}
            <span className="truncate">{att.filename}</span>
          </span>
        )}
        <span className={`text-xs flex-shrink-0 whitespace-nowrap ${error ? 'text-danger' : 'text-text-tertiary'}`}>
          {errorText}
        </span>
        {isLink && ready && (
          <span className="flex-shrink-0 text-[10px] font-medium uppercase tracking-wide text-primary bg-primary/10 rounded px-1.5 py-0.5">
            {t('mail_attach_link_badge', { defaultValue: 'lien' })}
          </span>
        )}
      </div>

      {/* ── Right side: progress rectangle while reading, else remove × ──────── */}
      {reading ? (
        <div className="flex items-center gap-1.5 flex-shrink-0">
          <div className="h-4 w-2/5 min-w-[120px] max-w-[240px] border border-border rounded-sm bg-white overflow-hidden">
            <div
              className="h-full bg-primary/30 transition-[width] duration-150"
              style={{ width: `${att.progress}%` }}
            />
          </div>
          {/* Cancel the read/upload in flight — revealed on hover, Gmail-style. */}
          <button
            type="button"
            onClick={() => onRemove(att.id)}
            title={t('common_cancel', { defaultValue: 'Annuler' })}
            className="p-0.5 rounded-full text-text-tertiary hover:text-danger hover:bg-danger/10 opacity-0 group-hover:opacity-100 transition"
          >
            <X size={14} />
          </button>
        </div>
      ) : (
        <button
          type="button"
          onClick={() => onRemove(att.id)}
          title={t('mail_remove_attachment', { defaultValue: 'Retirer' })}
          className="flex-shrink-0 p-0.5 rounded-full text-text-tertiary hover:text-danger hover:bg-danger/10 transition-colors"
        >
          <X size={15} />
        </button>
      )}
    </div>
  )
}

// ── Attachment bar: full-width bands stacked + the local-blob preview surface ──
// Clicking a blue name previews (image/pdf/text) or downloads a local file, or —
// for a Drive-link attachment — opens its public URL in a new tab.
export default function AttachmentBar({ attachments, onRemove, objectUrl }: {
  attachments: ComposeAttachment[]
  onRemove:    (id: string) => void
  objectUrl:   (att: ComposeAttachment) => string
}) {
  const [preview, setPreview] = useState<PreviewSource | null>(null)

  const open = (att: ComposeAttachment) => {
    if (att.status !== 'ready') return
    // Drive-link attachment: open the public share URL.
    if (att.kind === 'link') {
      if (att.driveUrl) window.open(att.driveUrl, '_blank', 'noopener')
      return
    }
    const url = objectUrl(att)
    if (isPreviewable(att.mime)) {
      setPreview({ url, name: att.filename, mime: att.mime })
    } else {
      // Not previewable → download via a transient anchor on the object URL.
      const a = document.createElement('a')
      a.href = url
      a.download = att.filename
      document.body.appendChild(a)
      a.click()
      a.remove()
    }
  }

  if (!attachments.length) return null
  return (
    <div className="flex flex-col gap-1.5 px-4 py-2 border-t border-border flex-shrink-0">
      {attachments.map(att => (
        <AttachmentChip key={att.id} att={att} onOpen={open} onRemove={onRemove} />
      ))}
      {preview && <AttachmentPreview preview={preview} onClose={() => setPreview(null)} />}
    </div>
  )
}
