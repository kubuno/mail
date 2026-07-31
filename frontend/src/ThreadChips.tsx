import { useTranslation } from 'react-i18next'
import type { Thread } from './api'
import { leafName } from './MailLabelItem'

/** System folders worth a chip, in Gmail's order. */
const FOLDER_CHIPS: { folder: string; key: string; fallback: string }[] = [
  { folder: 'inbox',  key: 'folder_inbox',  fallback: 'Boîte de réception' },
  { folder: 'sent',   key: 'folder_sent',   fallback: 'Messages envoyés' },
  { folder: 'drafts', key: 'folder_drafts', fallback: 'Brouillons' },
  { folder: 'spam',   key: 'folder_spam',   fallback: 'Spam' },
  { folder: 'trash',  key: 'folder_trash',  fallback: 'Corbeille' },
]

/**
 * Chips shown ahead of a row's subject: which system folders the thread sits
 * in, then its user labels. The chip describing the view you are already in is
 * dropped — inside the inbox every row would otherwise say "Inbox", and inside
 * a label every row would repeat that label.
 */
export default function ThreadChips({
  thread, currentFolder, currentLabelId,
}: {
  thread:         Thread
  currentFolder:  string
  currentLabelId: string | null
}) {
  const { t } = useTranslation('mail')

  const folders = (thread.folders ?? []).filter(f => f !== currentFolder)
  const labels  = (thread.labels  ?? []).filter(l => l.id !== currentLabelId)

  const folderChips = FOLDER_CHIPS.filter(f => folders.includes(f.folder))
  if (!folderChips.length && !labels.length) return null

  return (
    <>
      {folderChips.map(f => (
        <Chip key={f.folder} label={t(f.key, { defaultValue: f.fallback })} />
      ))}
      {labels.map(l => (
        <Chip key={l.id} label={leafName(l.name)} color={l.color} />
      ))}
    </>
  )
}

function Chip({ label, color }: { label: string; color?: string | null }) {
  const background = color ?? '#ddd'
  return (
    <span
      className="text-[11px] leading-none px-1.5 py-1 rounded-[4px] flex-shrink-0 max-w-[160px] truncate"
      style={{ backgroundColor: background, color: readableInk(background) }}
      title={label}
    >
      {label}
    </span>
  )
}

/** Black or white text, whichever holds contrast on the chip background. */
function readableInk(hex: string) {
  const h = hex.replace('#', '')
  const full = h.length === 3 ? h.split('').map(c => c + c).join('') : h
  const r = parseInt(full.slice(0, 2), 16)
  const g = parseInt(full.slice(2, 4), 16)
  const b = parseInt(full.slice(4, 6), 16)
  if ([r, g, b].some(Number.isNaN)) return '#202124'
  return (r * 299 + g * 587 + b * 114) / 1000 > 150 ? '#202124' : '#ffffff'
}
