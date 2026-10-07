/**
 * Items of the shell's "New" button for the mail module.
 *
 * Contributed as DATA (`MenuItem[]` from @ui) to the `shell.new-actions`
 * extension point — the shell renders them with the project's MenuDropdown.
 * Full set of compose-related actions (Gmail-style « + » menu): a plain new
 * message, an encrypted one, one from a template, then the create-things
 * actions (label / filter / distribution list / scheduled).
 *
 * `newActionItems` is evaluated when the menu OPENS, outside any React
 * component: no hooks here (store via `getState()`, i18n via `i18n.t`).
 */
import { Pencil, Lock, FileText, Tag, Filter, Users, Clock } from 'lucide-react'
import { i18n } from '@kubuno/sdk'
import type { MenuItem } from '@ui'
import { useMailStore } from './store'

function openBlank(extra: { secure?: boolean; schedule?: boolean; fullscreen?: boolean } = {}): void {
  const { setComposeInitial, setComposeOpen } = useMailStore.getState()
  setComposeInitial({ to: [], cc: [], subject: '', bodyHtml: '', ...extra })
  setComposeOpen(true)
}

export function newActionItems(): MenuItem[] {
  if (!window.location.pathname.startsWith('/mail')) return []

  const t = (key: string, defaultValue?: string) => i18n.t(`mail:${key}`, { defaultValue })

  return [
    {
      type: 'action',
      label: t('new_message'),
      icon: <Pencil size={16} />,
      onClick: () => useMailStore.getState().setComposeOpen(true),
    },
    { type: 'separator' },
    {
      type: 'action',
      label: t('mail_new_secure', 'Message chiffré (PGP)'),
      icon: <Lock size={16} />,
      onClick: () => openBlank({ secure: true }),
    },
    {
      type: 'action',
      label: t('mail_new_from_template', "À partir d'un modèle"),
      icon: <FileText size={16} />,
      onClick: () => useMailStore.getState().setTemplatesOpen(true),
    },
    { type: 'separator' },
    {
      type: 'action',
      label: t('label_create', 'Nouveau libellé'),
      icon: <Tag size={16} />,
      onClick: () => useMailStore.getState().requestCreateLabel(),
    },
    {
      type: 'action',
      label: t('mail_new_filter', 'Nouveau filtre / règle'),
      icon: <Filter size={16} />,
      onClick: () => useMailStore.getState().requestNewFilter(),
    },
    {
      type: 'action',
      label: t('mail_new_group', 'Nouvelle liste de diffusion'),
      icon: <Users size={16} />,
      onClick: () => useMailStore.getState().setGroupsOpen(true),
    },
    {
      type: 'action',
      label: t('mail_new_scheduled', 'Message programmé'),
      icon: <Clock size={16} />,
      onClick: () => openBlank({ schedule: true }),
    },
  ]
}
