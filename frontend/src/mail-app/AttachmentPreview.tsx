import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Download, X, Loader2 } from 'lucide-react'
import PdfViewerModal from '../PdfViewerModal'
import { sanitizeDisplayText } from './senderSafety'

export type PreviewSource = { url: string; name: string; mime: string }

/**
 * Our own preview surfaces for an attachment, used when Drive — which owns the
 * real viewers — is not installed. PDF goes to the module's viewer, images to a
 * full-screen lightbox, and plain text to a framed reader. Works from either a
 * server URL or a local object URL (a not-yet-sent compose attachment).
 */
export default function AttachmentPreview({ preview, onClose }: {
  preview: PreviewSource
  onClose: () => void
}) {
  const { t } = useTranslation('mail')
  // Defence in depth: callers already hand over a cleaned name, but this
  // overlay paints it over a full-screen surface where a reversed name is the
  // most convincing. Never trust the caller for attacker-controlled text.
  const name = sanitizeDisplayText(preview.name)

  return (
    <>
      {preview.mime === 'application/pdf' && (
        <PdfViewerModal url={preview.url} filename={name} onClose={onClose} />
      )}
      {preview.mime.startsWith('image/') && (
        <div className="fixed inset-0 z-[60] bg-black/90 flex flex-col" onClick={onClose}>
          <div className="flex items-center justify-between px-4 h-14 text-white flex-shrink-0"
               onClick={e => e.stopPropagation()}>
            <span className="text-sm truncate">{name}</span>
            <div className="flex items-center gap-1">
              <button onClick={() => window.open(preview.url, '_blank', 'noopener')}
                title={t('download', { defaultValue: 'Télécharger' })}
                className="w-10 h-10 flex items-center justify-center rounded-full hover:bg-white/10">
                <Download size={20} />
              </button>
              <button onClick={onClose} title={t('common_close')}
                className="w-10 h-10 flex items-center justify-center rounded-full hover:bg-white/10">
                <X size={20} />
              </button>
            </div>
          </div>
          <div className="flex-1 flex items-center justify-center p-6 overflow-auto">
            <img src={preview.url} alt={name}
                 onClick={e => e.stopPropagation()}
                 className="max-w-full max-h-full object-contain" />
          </div>
        </div>
      )}
      {preview.mime.startsWith('text/') && (
        <TextPreview preview={preview} onClose={onClose} />
      )}
    </>
  )
}

// Renders a text attachment inside a <pre>. We fetch the bytes ourselves rather
// than framing the URL: a hardened host CSP can block `blob:`/documents in
// `frame-src`, and fetch() of an object URL isn't subject to it.
function TextPreview({ preview, onClose }: { preview: PreviewSource; onClose: () => void }) {
  const { t } = useTranslation('mail')
  const name = sanitizeDisplayText(preview.name)
  const [text, setText] = useState<string | null>(null)
  const [error, setError] = useState(false)

  useEffect(() => {
    let cancelled = false
    fetch(preview.url)
      .then(r => r.text())
      .then(body => { if (!cancelled) setText(body) })
      .catch(() => { if (!cancelled) setError(true) })
    return () => { cancelled = true }
  }, [preview.url])

  return (
    <div className="fixed inset-0 z-[60] bg-black/70 flex flex-col p-6" onClick={onClose}>
      <div className="flex flex-col w-full max-w-3xl mx-auto flex-1 min-h-0 bg-white rounded-xl overflow-hidden"
           onClick={e => e.stopPropagation()}>
        <div className="flex items-center justify-between px-4 h-12 border-b border-border flex-shrink-0">
          <span className="text-sm text-text-primary truncate">{name}</span>
          <div className="flex items-center gap-1">
            <a href={preview.url} download={name}
               title={t('download', { defaultValue: 'Télécharger' })}
               className="w-9 h-9 flex items-center justify-center rounded-full hover:bg-surface-2 text-text-tertiary">
              <Download size={18} />
            </a>
            <button onClick={onClose} title={t('common_close')}
              className="w-9 h-9 flex items-center justify-center rounded-full hover:bg-surface-2 text-text-tertiary">
              <X size={18} />
            </button>
          </div>
        </div>
        <div className="flex-1 min-h-0 overflow-auto p-4">
          {error
            ? <p className="text-sm text-text-tertiary">{t('mail_preview_failed', { defaultValue: 'Aperçu indisponible.' })}</p>
            : text === null
              ? <Loader2 size={20} className="animate-spin text-text-tertiary" />
              : <pre className="text-xs text-text-primary whitespace-pre-wrap break-words font-mono">{text}</pre>}
        </div>
      </div>
    </div>
  )
}
