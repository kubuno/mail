import { useTranslation } from 'react-i18next'
import {
  Reply, Forward, Trash2, MailOpen, AlertCircle, Ban, ShieldAlert,
  Filter, Printer, Download, Code2,
} from 'lucide-react'
import { MenuDropdown, type MenuItem as UiMenuItem } from '@ui'

// ── Message actions menu ──────────────────────────────────────────────────────

export default function MessageActionsMenu({
  anchorRect, onClose, onReply, onForward, onDelete, onMarkUnread, onBlock,
  onSpam, onFilterSimilar, onDownload, onShowOriginal,
}: {
  anchorRect:   DOMRect
  onClose:      () => void
  onReply:      () => void
  onForward:    () => void
  onDelete:     () => void
  onMarkUnread: () => void
  onBlock:      () => void
  onSpam:           () => void
  onFilterSimilar:  () => void
  onDownload:       () => void
  onShowOriginal:   () => void
}) {
  const { t } = useTranslation('mail')
  // MenuDropdown from @ui: anchored dropdown on desktop, bottom sheet on touch.
  const menuW = 285
  const items: UiMenuItem[] = [
    { type: 'action', icon: <Reply size={15} />,       label: t('mail_reply'),       onClick: onReply },
    { type: 'action', icon: <Forward size={15} />,     label: t('mail_forward'),     onClick: onForward },
    { type: 'separator' },
    { type: 'action', icon: <Trash2 size={15} />,      label: t('delete'),           onClick: onDelete, danger: true },
    { type: 'action', icon: <MailOpen size={15} />,    label: t('mail_mark_unread'), onClick: onMarkUnread },
    { type: 'separator' },
    { type: 'action', icon: <AlertCircle size={15} />, label: t('spam_report'),      onClick: onSpam },
    { type: 'action', icon: <Ban size={15} />,         label: t('block_sender', { defaultValue: 'Bloquer l\'expéditeur' }), onClick: onBlock },
    // Phishing = spam + blocked sender (trains the Bayesian filter AND cuts the source).
    { type: 'action', icon: <ShieldAlert size={15} />, label: t('mail_report_phishing'), onClick: () => { onBlock(); onSpam() } },
    { type: 'separator' },
    { type: 'action', icon: <Filter size={15} />,      label: t('mail_filter_similar'),  onClick: onFilterSimilar },
    { type: 'action', icon: <Printer size={15} />,     label: t('print'),                onClick: () => window.print() },
    { type: 'action', icon: <Download size={15} />,    label: t('mail_download_message'), onClick: onDownload },
    { type: 'action', icon: <Code2 size={15} />,       label: t('mail_show_original'),   onClick: onShowOriginal },
  ]

  return (
    <MenuDropdown
      pos={{ top: anchorRect.bottom + 4, left: Math.max(8, anchorRect.right - menuW), minWidth: menuW }}
      onClose={onClose}
      items={items}
    />
  )
}
