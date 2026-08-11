import * as DropdownMenu from '@radix-ui/react-dropdown-menu'
import { Pencil, Lock, FileText, Tag, Filter, Users, Clock } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { useMailStore } from './store'

// Content of the shell's default "New" button dropdown for the mail module.
// It offers the full set of compose-related actions (Gmail-style « + » menu):
// a plain new message, an encrypted one, one from a template, a pop-out window,
// then the create-things actions (label / filter / distribution list / scheduled).
const ITEM_CLASS =
  'flex items-center gap-3 w-full px-3 py-2 text-sm text-text-primary ' +
  'hover:bg-surface-1 cursor-pointer outline-none'
const SEP_CLASS = 'my-1 h-px bg-border'

function Item({ icon, label, onSelect }: { icon: React.ReactNode; label: string; onSelect: () => void }) {
  return (
    <DropdownMenu.Item onSelect={onSelect} className={ITEM_CLASS}>
      <span className="text-text-secondary flex-shrink-0">{icon}</span>
      {label}
    </DropdownMenu.Item>
  )
}

export default function MailCreateMenu() {
  const { t } = useTranslation('mail')
  const {
    setComposeOpen, setComposeInitial,
    requestCreateLabel, requestNewFilter, setTemplatesOpen, setGroupsOpen,
  } = useMailStore()

  const openBlank = (extra: { secure?: boolean; schedule?: boolean; fullscreen?: boolean } = {}) => {
    setComposeInitial({ to: [], cc: [], subject: '', bodyHtml: '', ...extra })
    setComposeOpen(true)
  }

  return (
    <>
      <Item icon={<Pencil size={16} />} label={t('new_message')} onSelect={() => setComposeOpen(true)} />

      <DropdownMenu.Separator className={SEP_CLASS} />

      <Item
        icon={<Lock size={16} />}
        label={t('mail_new_secure', { defaultValue: 'Message chiffré (PGP)' })}
        onSelect={() => openBlank({ secure: true })}
      />
      <Item
        icon={<FileText size={16} />}
        label={t('mail_new_from_template', { defaultValue: "À partir d'un modèle" })}
        onSelect={() => setTemplatesOpen(true)}
      />

      <DropdownMenu.Separator className={SEP_CLASS} />

      <Item
        icon={<Tag size={16} />}
        label={t('label_create', { defaultValue: 'Nouveau libellé' })}
        onSelect={() => requestCreateLabel()}
      />
      <Item
        icon={<Filter size={16} />}
        label={t('mail_new_filter', { defaultValue: 'Nouveau filtre / règle' })}
        onSelect={() => requestNewFilter()}
      />
      <Item
        icon={<Users size={16} />}
        label={t('mail_new_group', { defaultValue: 'Nouvelle liste de diffusion' })}
        onSelect={() => setGroupsOpen(true)}
      />
      <Item
        icon={<Clock size={16} />}
        label={t('mail_new_scheduled', { defaultValue: 'Message programmé' })}
        onSelect={() => openBlank({ schedule: true })}
      />
    </>
  )
}
