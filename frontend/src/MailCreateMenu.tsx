import * as DropdownMenu from '@radix-ui/react-dropdown-menu'
import { Pencil } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { useMailStore } from './store'

// Content of the shell's default "New" button dropdown for the mail module:
// a single "New message" action that opens the compose window.
const ITEM_CLASS =
  'flex items-center gap-3 w-full px-3 py-2 text-sm text-text-primary ' +
  'hover:bg-surface-1 cursor-pointer outline-none'

export default function MailCreateMenu() {
  const { t } = useTranslation('mail')
  const setComposeOpen = useMailStore((s) => s.setComposeOpen)

  return (
    <DropdownMenu.Item onSelect={() => setComposeOpen(true)} className={ITEM_CLASS}>
      <Pencil size={16} className="text-text-secondary" />
      {t('new_message')}
    </DropdownMenu.Item>
  )
}
