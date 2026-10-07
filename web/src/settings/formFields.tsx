import { Eye, EyeOff } from 'lucide-react'

// ── Account form field primitives ─────────────────────────────────────────────
// Plain inputs shared by the account dialog (label above the control).

const LABEL_CLASS = 'block text-xs font-medium text-text-secondary mb-1'

const INPUT_CLASS =
  'w-full border border-border rounded-lg px-3 py-2 text-sm text-text-primary ' +
  'focus:outline-none focus:ring-2 focus:ring-primary/30'

/** Compact input without the focus ring (used for numeric port fields). */
const PLAIN_INPUT_CLASS = 'w-full border border-border rounded-lg px-3 py-2 text-sm'

export function TextField({ label, value, onChange, type = 'text', className }: {
  label:      string
  value:      string
  onChange:   (v: string) => void
  type?:      string
  className?: string
}) {
  return (
    <div className={className}>
      <label className={LABEL_CLASS}>{label}</label>
      <input
        type={type}
        value={value}
        onChange={e => onChange(e.target.value)}
        className={INPUT_CLASS}
      />
    </div>
  )
}

export function NumberField({ label, value, onChange, className }: {
  label:      string
  value:      number | undefined
  onChange:   (v: number) => void
  className?: string
}) {
  return (
    <div className={className}>
      <label className={LABEL_CLASS}>{label}</label>
      <input
        type="number"
        value={value}
        onChange={e => onChange(Number(e.target.value))}
        className={PLAIN_INPUT_CLASS}
      />
    </div>
  )
}

/** Password input with a reveal toggle; the visibility state is owned by the caller. */
export function PasswordField({
  label, value, onChange, placeholder, show, onToggleShow, className,
}: {
  label:        string
  value:        string
  onChange:     (v: string) => void
  placeholder?: string
  show:         boolean
  onToggleShow: () => void
  className?:   string
}) {
  return (
    <div className={className}>
      <label className={LABEL_CLASS}>{label}</label>
      <div className="relative">
        <input
          type={show ? 'text' : 'password'}
          value={value}
          onChange={e => onChange(e.target.value)}
          placeholder={placeholder}
          className="w-full border border-border rounded-lg px-3 py-2 pr-10 text-sm"
        />
        <button
          type="button"
          onClick={onToggleShow}
          className="absolute right-3 top-1/2 -translate-y-1/2 text-text-tertiary"
        >
          {show ? <EyeOff size={14} /> : <Eye size={14} />}
        </button>
      </div>
    </div>
  )
}
