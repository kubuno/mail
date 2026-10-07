import { useState, useRef, useEffect } from 'react'
import { useTranslation } from 'react-i18next'
import { MenuDropdown, DatePicker, Button, type MenuItem, type MenuDropdownPos } from '@ui'
import { ChevronDown } from 'lucide-react'

// The « Envoyer » split button: immediate send + a chevron opening the
// "schedule send" menu (presets + custom date/time dialog). Extracted verbatim
// from ComposeWindow — positioning, classes and behaviour are unchanged.
export default function SendButton({
  onSendNow, onSchedule, disabled, pending, autoOpen,
}: {
  onSendNow: () => void
  onSchedule: (iso: string) => void
  disabled: boolean
  pending: boolean
  autoOpen: boolean
}) {
  const { t } = useTranslation('mail')
  const scheduleBtnRef = useRef<HTMLButtonElement>(null)
  const [sendMenuPos, setSendMenuPos] = useState<MenuDropdownPos | null>(null)
  // Custom "pick a date & time" scheduling dialog.
  const [customSchedule, setCustomSchedule] = useState(false)
  const [customDT, setCustomDT] = useState<string | null>(null)

  // Position of the "Schedule send" menu: below the send group and left-aligned
  // with the « Envoyer » button when there is room, otherwise flipped above it —
  // never covering the button, always kept on-screen.
  const scheduleMenuPos = (): MenuDropdownPos => {
    const group = scheduleBtnRef.current?.parentElement
    const r = (group ?? scheduleBtnRef.current)?.getBoundingClientRect()
    if (!r) return { top: 0, left: 0 }
    const GAP = 6, MENU_H = 168, MIN_W = 232
    const below = r.bottom + GAP
    const top = below + MENU_H <= window.innerHeight ? below : Math.max(8, r.top - MENU_H - GAP)
    const left = Math.max(8, Math.min(r.left, window.innerWidth - MIN_W))
    return { top, left }
  }

  const schedulePresets = () => {
    const now = new Date()
    const later   = new Date(now); later.setHours(now.getHours() + 2, 0, 0, 0)
    const tom     = new Date(now); tom.setDate(now.getDate() + 1); tom.setHours(8, 0, 0, 0)
    const mon     = new Date(now); mon.setDate(now.getDate() + ((8 - now.getDay()) % 7 || 7)); mon.setHours(8, 0, 0, 0)
    return [
      { label: t('schedule_later',    { defaultValue: 'Plus tard (2 h)' }),     at: later.toISOString() },
      { label: t('schedule_tomorrow', { defaultValue: 'Demain matin' }),         at: tom.toISOString() },
      { label: t('schedule_monday',   { defaultValue: 'Lundi matin' }),          at: mon.toISOString() },
    ]
  }

  // « Message programmé »: pop the schedule-send menu open once the composer is
  // mounted (the action bar's chevron button anchors it).
  useEffect(() => {
    if (!autoOpen) return
    const id = requestAnimationFrame(() => {
      if (!scheduleBtnRef.current) return
      setSendMenuPos(scheduleMenuPos())
    })
    return () => cancelAnimationFrame(id)
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  return (
    <div className="relative flex items-stretch flex-shrink-0 mr-2">
      <button
        onClick={onSendNow}
        disabled={disabled}
        className="flex items-center gap-2 h-9 pl-5 pr-4 text-[14px] font-medium bg-primary text-white rounded-l-lg hover:bg-primary-hover disabled:opacity-50 transition-colors"
      >
        {t('mail_send')}
      </button>
      <button
        ref={scheduleBtnRef}
        onClick={() => setSendMenuPos(p => p ? null : scheduleMenuPos())}
        disabled={pending || disabled}
        title={t('schedule_send', { defaultValue: 'Programmer l\'envoi' })}
        className="flex items-center h-9 px-1.5 bg-primary text-white rounded-r-lg border-l border-white/25 hover:bg-primary-hover disabled:opacity-50 transition-colors ml-px"
      >
        <ChevronDown size={14} />
      </button>
      {sendMenuPos && (
        <MenuDropdown
          pos={{ ...sendMenuPos, minWidth: 224 }}
          onClose={() => setSendMenuPos(null)}
          items={[
            { type: 'label', text: t('schedule_send', { defaultValue: 'Programmer l\'envoi' }) },
            ...schedulePresets().map<MenuItem>(p => ({ type: 'action', label: p.label, onClick: () => onSchedule(p.at) })),
            { type: 'separator' },
            { type: 'action', label: t('schedule_custom', { defaultValue: 'Date et heure personnalisées…' }),
              onClick: () => { setSendMenuPos(null); setCustomDT(null); setCustomSchedule(true) } },
          ]}
        />
      )}
      {customSchedule && (
        <div className="fixed inset-0 z-[9999] bg-black/30 flex items-center justify-center p-4"
          onClick={() => setCustomSchedule(false)}>
          <div className="bg-white rounded-xl shadow-xl w-full max-w-[360px] p-5" onClick={e => e.stopPropagation()}>
            <h3 className="text-sm font-medium text-text-primary mb-3">
              {t('schedule_custom_title', { defaultValue: 'Programmer à une date précise' })}
            </h3>
            <DatePicker
              mode="datetime"
              value={customDT}
              onChange={setCustomDT}
              minDate={new Date().toISOString().slice(0, 10)}
              minuteStep={5}
              clearable
              placeholder={t('schedule_custom_ph', { defaultValue: 'Choisir une date et une heure' })}
            />
            <div className="flex justify-end gap-2 mt-4">
              <Button variant="ghost" onClick={() => setCustomSchedule(false)}>
                {t('common_cancel', { defaultValue: 'Annuler' })}
              </Button>
              <Button
                disabled={!customDT || pending}
                onClick={() => { if (customDT) { onSchedule(new Date(customDT).toISOString()); setCustomSchedule(false) } }}
              >
                {t('schedule_send', { defaultValue: 'Programmer l\'envoi' })}
              </Button>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}
