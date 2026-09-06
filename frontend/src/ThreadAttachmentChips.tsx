import { useTranslation } from 'react-i18next'
import { mailApi, type ThreadAttachment } from './api'
import { sanitizeDisplayText } from './mail-app/senderSafety'

/** How many chips fit before we collapse the rest into "and N more". */
const MAX_CHIPS = 3

/**
 * File-type badge: one uniform rounded square per chip, coloured by family and
 * carrying the extension.
 *
 * Type-specific glyphs were tried first and read badly — every icon had its own
 * optical weight and width, so a row of chips looked ragged. A single shape
 * with a colour and a label stays even whatever the file type.
 */
function badgeFor(mime: string, name: string) {
  const ext = (name.split('.').pop() ?? '').toLowerCase().slice(0, 4)
  if (mime === 'application/pdf' || ext === 'pdf') return { bg: '#d93025', label: 'PDF' }
  if (mime.startsWith('image/'))                   return { bg: '#1e8e3e', label: ext || 'img' }
  if (mime.startsWith('video/'))                   return { bg: '#a142f4', label: ext || 'vid' }
  if (mime.startsWith('audio/'))                   return { bg: '#f9ab00', label: ext || 'son' }
  if (/zip|tar|rar|7z|gzip/.test(mime))            return { bg: '#5f6368', label: ext || 'zip' }
  if (/word|document/.test(mime))                  return { bg: '#1a73e8', label: ext || 'doc' }
  if (/sheet|excel|csv/.test(mime))                return { bg: '#0f9d58', label: ext || 'xls' }
  return { bg: '#5f6368', label: ext || '···' }
}

/**
 * Attachment chips under a conversation row. Clicking one opens the preview
 * when the type can be rendered, and downloads it otherwise.
 */
export default function ThreadAttachmentChips({
  attachments, onPreview,
}: {
  attachments: ThreadAttachment[]
  onPreview: (att: ThreadAttachment, url: string) => void
}) {
  const { t } = useTranslation('mail')
  if (!attachments.length) return null

  const shown = attachments.slice(0, MAX_CHIPS)
  const rest  = attachments.length - shown.length

  return (
    // The vertical padding is carried here rather than on the row: chips sat
    // flush against the separator below, which made the whole list feel packed.
    // No click sink on the strip itself: it stretches across the whole subject
    // column, so swallowing clicks here made the entire attachment line of a row
    // dead — half its height opened nothing. Each chip below stops propagation
    // on its own, which is what keeps "preview this file" from also opening the
    // conversation; everywhere else the click belongs to the row.
    <div className="flex items-center gap-2.5 flex-wrap pt-0.5 pb-1.5">
      {shown.map(att => {
        const badge = badgeFor(att.mime, att.name)
        // Bidi overrides in a file name would flip the whole chip row, not just
        // the label — strip them before the name reaches the DOM.
        const label = sanitizeDisplayText(att.name)
        return (
          <button
            key={`${att.message_id}-${att.index}`}
            title={label}
            onClick={e => {
              e.stopPropagation()
              onPreview(att, mailApi.attachmentUrl(att.message_id, att.index))
            }}
            // Transparent fill so the chip sits on the row's own tint (read rows
            // are blue-grey, unread ones white) instead of punching a white hole
            // through it; the outline alone carries the shape.
            className="flex items-center gap-2 h-7 pl-1.5 pr-3 max-w-[240px]
                       rounded-[4px] border border-[#dadce0] bg-transparent
                       text-xs text-[#3c4043] hover:bg-black/[0.04] transition-colors"
          >
            <span
              className="w-[18px] h-[18px] rounded-[3px] flex items-center justify-center flex-shrink-0
                         text-[7px] font-bold uppercase text-white leading-none tracking-tight"
              style={{ backgroundColor: badge.bg }}
            >
              {badge.label}
            </span>
            <span className="truncate">{label}</span>
          </button>
        )
      })}
      {rest > 0 && (
        <span className="text-xs text-[#5f6368]">
          {t('mail_and_n_more', { count: rest, defaultValue: `et ${rest} de plus` })}
        </span>
      )}
    </div>
  )
}
