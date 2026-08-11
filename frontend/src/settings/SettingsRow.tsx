import React from 'react'
import { Radio } from '@ui'

// ── Layout helpers ────────────────────────────────────────────────────────────

/** Two-column settings row: label (+ optional description) on the left, control on the right. */
export function SettingsRow({ label, description, children }: {
  label: string; description?: string; children: React.ReactNode
}) {
  return (
    <div className="flex items-start gap-8 py-4 border-b border-[#e8eaed] last:border-0">
      <div className="w-60 flex-shrink-0">
        <p className="text-sm text-[#202124] font-normal">{label}</p>
        {description && (
          <p className="text-xs text-text-tertiary mt-0.5 leading-relaxed">{description}</p>
        )}
      </div>
      <div className="flex-1">{children}</div>
    </div>
  )
}

/** Vertical list of exclusive options built on the @ui Radio primitive. */
export function RadioGroup({ options, value, onChange }: {
  name?: string
  options: { value: string; label: string }[]
  value: string
  onChange: (v: string) => void
}) {
  return (
    <div className="space-y-2">
      {options.map(opt => (
        <Radio
          key={opt.value}
          checked={value === opt.value}
          onChange={() => onChange(opt.value)}
          label={opt.label}
        />
      ))}
    </div>
  )
}
