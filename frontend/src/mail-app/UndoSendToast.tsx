import { useEffect, useState } from 'react'
import { Send, Undo2 } from 'lucide-react'
import { useUndoSendStore } from '../undoSendStore'

// ── "Undo send" countdown toast ───────────────────────────────────────────────
// Reuses the mechanics of drive's delete countdown (card + progress bar that
// empties over `duration` ms), restyled for mail (primary blue accent, a
// positive "sent" action rather than a destructive one).

export default function UndoSendToast({ label, undoLabel, onCancel }: {
  label: string; undoLabel: string; onCancel: () => void
}) {
  const duration = useUndoSendStore(s => s.duration)
  // Bar emptying from 100 % → 0 % over `duration` ms (linear CSS transition).
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
