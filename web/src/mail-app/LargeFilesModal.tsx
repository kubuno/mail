import { useTranslation } from 'react-i18next'
import { X, HardDrive } from 'lucide-react'
import type { ComposeAttachment } from './composeAttachments'

const fmtSize = (n: number) =>
  n < 1024 ? `${n} o` : n < 1048576 ? `${Math.round(n / 1024)} Ko` : `${(n / 1048576).toFixed(1)} Mo`

// ── « Ajout des fichiers… » modal (Gmail parity) ─────────────────────────────
// Shown while one or more oversized files are being uploaded to Drive as public
// links. Overlay above the composer with a card: title, a subtitle carrying the
// admin size limit, a scrollable list (name + size + upload progress bar + × to
// cancel that file), and a « Annuler » button that cancels every upload and
// closes. It opens as soon as a file exceeds the limit and closes once every
// large upload has finished (or been cancelled) — the parent renders it only
// while `files` is non-empty.
export default function LargeFilesModal({ files, maxMb, onCancelOne, onCancelAll }: {
  files:       ComposeAttachment[]
  maxMb:       number
  onCancelOne: (id: string) => void
  onCancelAll: () => void
}) {
  const { t } = useTranslation('mail')
  if (!files.length) return null

  return (
    <div className="fixed inset-0 z-[60] flex items-center justify-center bg-black/40 p-6">
      <div className="w-full max-w-md bg-white rounded-xl shadow-2xl flex flex-col overflow-hidden">
        {/* ── Header ─────────────────────────────────────────────────────────── */}
        <div className="px-5 pt-4 pb-3 border-b border-border">
          <h2 className="text-base font-medium text-text-primary">
            {t('mail_large_files_title', { defaultValue: 'Ajout des fichiers…' })}
          </h2>
          <p className="mt-1 text-sm text-text-secondary">
            {t('mail_large_files_desc', {
              defaultValue: 'La taille de vos fichiers dépasse {{n}} Mo. Ils seront envoyés sous forme de liens Drive.',
              n: maxMb,
            })}
          </p>
        </div>

        {/* ── File list ──────────────────────────────────────────────────────── */}
        <div className="max-h-72 overflow-y-auto px-5 py-3 flex flex-col gap-3">
          {files.map(f => (
            <div key={f.id} className="flex items-center gap-3">
              <HardDrive size={16} className="flex-shrink-0 text-primary" />
              <div className="flex-1 min-w-0">
                <div className="flex items-baseline gap-1.5">
                  <span className="text-sm text-text-primary truncate" title={f.filename}>{f.filename}</span>
                  <span className="text-xs text-text-tertiary flex-shrink-0 whitespace-nowrap">({fmtSize(f.size)})</span>
                </div>
                <div className="mt-1 h-1.5 w-full rounded-full bg-surface-2 overflow-hidden">
                  <div
                    className="h-full bg-primary transition-[width] duration-150"
                    style={{ width: `${f.progress}%` }}
                  />
                </div>
              </div>
              <button
                type="button"
                onClick={() => onCancelOne(f.id)}
                title={t('common_cancel', { defaultValue: 'Annuler' })}
                className="flex-shrink-0 p-1 rounded-full text-text-tertiary hover:text-danger hover:bg-danger/10 transition-colors"
              >
                <X size={15} />
              </button>
            </div>
          ))}
        </div>

        {/* ── Footer ─────────────────────────────────────────────────────────── */}
        <div className="px-5 py-3 border-t border-border flex justify-end">
          <button
            type="button"
            onClick={onCancelAll}
            className="px-4 h-9 text-sm text-text-secondary rounded-lg hover:bg-surface-2 transition-colors"
          >
            {t('common_cancel', { defaultValue: 'Annuler' })}
          </button>
        </div>
      </div>
    </div>
  )
}
