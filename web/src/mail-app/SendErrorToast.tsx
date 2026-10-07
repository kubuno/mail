import { useEffect } from 'react'
import { AlertTriangle, X } from 'lucide-react'

// ── Delayed-send failure toast ────────────────────────────────────────────────
// The "undo send" design fires the real send after the compose window is gone,
// so a failure has nowhere to surface. This shows it: a dismissible danger card,
// self-clearing after a while so it never lingers.

export default function SendErrorToast({ message, onClose }: {
  message: string; onClose: () => void
}) {
  useEffect(() => {
    const t = setTimeout(onClose, 8000)
    return () => clearTimeout(t)
  }, [message, onClose])

  return (
    <div className="fixed bottom-6 left-6 z-[100] w-80 rounded-xl border border-danger/40 bg-surface-0 shadow-lg overflow-hidden">
      <div className="flex items-start gap-3 px-4 py-3">
        <AlertTriangle size={18} className="text-danger shrink-0 mt-0.5" />
        <span className="flex-1 text-sm text-text-primary">{message}</span>
        <button onClick={onClose} className="text-text-tertiary hover:text-text-primary shrink-0" aria-label="Fermer">
          <X size={14} />
        </button>
      </div>
    </div>
  )
}
