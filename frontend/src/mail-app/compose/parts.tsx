import { useState } from 'react'
import type { Label } from '../../api'

// Web-safe families for the compose toolbar (applied via execCommand('fontName')).
export const MAIL_FONTS = ['Arial', 'Verdana', 'Trebuchet MS', 'Tahoma', 'Georgia', 'Times New Roman', 'Courier New', 'Comic Sans MS']
export const MAIL_SIZES = [8, 9, 10, 11, 12, 13, 14, 16, 18, 24, 36, 48]

export const EMOJIS = ['😀','😅','😉','😍','😘','😎','🤔','🙏','👍','👎','👏','🙌','🎉','🔥','✅','❌','⭐','❤️','💡','📎','📅','⏰']
export const COLORS = ['#202124','#d93025','#e8710a','#188038','#1a73e8','#9334e6','#c2185b','#5f6368']

export function ToolBtn({ onClick, title, children }: {
  onClick: () => void; title: string; children: React.ReactNode
}) {
  return (
    <button
      onMouseDown={e => { e.preventDefault(); onClick() }}
      title={title}
      className="w-7 h-7 flex items-center justify-center rounded hover:bg-black/10 text-text-primary transition-colors flex-shrink-0"
    >
      {children}
    </button>
  )
}

export function IconBtn({ onClick, title, children }: {
  onClick?: (e?: React.MouseEvent) => void; title: string; children: React.ReactNode
}) {
  return (
    <button
      onClick={onClick}
      title={title}
      className="p-1.5 rounded-full hover:bg-surface-2 text-text-tertiary transition-colors flex-shrink-0"
    >
      {children}
    </button>
  )
}

// Searchable label checklist for the "Libellé" submenu of the « ⋯ » menu.
export function LabelChecklist({ labels, checked, onToggle, placeholder }: {
  labels: Label[]; checked: Set<string>; onToggle: (id: string) => void; placeholder: string
}) {
  const [q, setQ] = useState('')
  const visible = labels.filter(l => !l.is_system && l.name.toLowerCase().includes(q.toLowerCase()))
  return (
    <div className="w-64 px-1 pb-1">
      <input
        autoFocus value={q} onChange={e => setQ(e.target.value)} placeholder={placeholder}
        className="w-full h-8 px-2 text-sm border-b border-border outline-none focus:border-primary mb-1"
      />
      <div className="max-h-56 overflow-y-auto">
        {visible.length === 0
          ? <div className="px-2 py-2 text-xs text-text-tertiary">—</div>
          : visible.map(l => (
            <label key={l.id} className="flex items-center gap-2 px-2 py-1.5 rounded hover:bg-surface-1 cursor-pointer text-sm text-text-primary">
              <input type="checkbox" checked={checked.has(l.id)} onChange={() => onToggle(l.id)} />
              <span className="truncate">{l.name}</span>
            </label>
          ))}
      </div>
    </div>
  )
}
