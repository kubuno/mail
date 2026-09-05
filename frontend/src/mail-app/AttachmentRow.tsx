import { useTranslation } from 'react-i18next'
import { FileText, Download, AlertTriangle } from 'lucide-react'
import { mailApi, Attachment } from '../api'
import { sanitizeFileName } from './senderSafety'

// ── Attachment row ─────────────────────────────────────────────────────────────

export default function AttachmentRow({
  att, index, messageId, onOpenPdf,
}: { att: Attachment; index: number; messageId: string; onOpenPdf: (url: string, name: string) => void }) {
  const { t } = useTranslation('mail')
  const url   = mailApi.attachmentUrl(messageId, index)
  const isPdf = att.mime === 'application/pdf'
  // A file name is displayed AND written to disk: a U+202E in it turns
  // "facture_gpj.exe" into "facture_exe.jpg" on screen. Strip the overrides
  // everywhere the name travels — label, tooltip, viewer title, download.
  const file  = sanitizeFileName(att.name)

  return (
    <div className="flex items-center gap-2 px-3 py-2 rounded-lg border border-border hover:bg-surface-1 transition-colors group">
      <FileText size={16} className="text-text-tertiary flex-shrink-0" />
      <button
        onClick={() => isPdf ? onOpenPdf(url, file.name) : window.open(url, '_blank')}
        title={file.name}
        className="flex-1 text-left text-sm text-text-primary truncate hover:text-primary transition-colors"
      >
        {file.name}
      </button>
      {/* The name carried hidden formatting characters: say so rather than
          silently showing a name that is not the one that was sent. */}
      {file.altered && (
        <span
          className="flex-shrink-0 text-[#b06000]"
          title={t('mail_attachment_name_cleaned', {
            defaultValue: 'Le nom de ce fichier contenait des caractères masqués (sens d’écriture inversé). Il est affiché nettoyé.',
          })}
        >
          <AlertTriangle size={14} />
        </span>
      )}
      <span className="text-xs text-text-tertiary flex-shrink-0">
        {att.size > 0
          ? att.size < 1024 * 1024
            ? t('mail_size_kb', { size: Math.round(att.size / 1024) })
            : t('mail_size_mb', { size: (att.size / 1024 / 1024).toFixed(1) })
          : ''}
      </span>
      <a
        href={url}
        download={file.name}
        onClick={e => e.stopPropagation()}
        className="p-1 rounded opacity-0 group-hover:opacity-100 hover:bg-surface-2 transition-all text-text-tertiary"
        title={t('mail_download')}
      >
        <Download size={14} />
      </a>
    </div>
  )
}
