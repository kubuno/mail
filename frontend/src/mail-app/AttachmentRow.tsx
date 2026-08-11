import { useTranslation } from 'react-i18next'
import { FileText, Download } from 'lucide-react'
import { mailApi, Attachment } from '../api'

// ── Attachment row ─────────────────────────────────────────────────────────────

export default function AttachmentRow({
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
