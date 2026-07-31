import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router-dom'
import { MoreVertical, Tag } from 'lucide-react'
import { MenuDropdown, useMenuDropdown, type MenuItem } from '@ui'
import type { Label, LabelListVisibility, LabelMsgVisibility } from './api'

/** Swatches offered in the "Label colour" submenu (Gmail-like palette). */
export const LABEL_PALETTE = [
  '#5f6368', '#8ab4f8', '#4285f4', '#c58af9', '#f28b82', '#ee675c',
  '#202124', '#1a73e8', '#1967d2', '#a142f4', '#d93025', '#b31412',
  '#e8eaed', '#fdd663', '#fbbc04', '#ccff90', '#a8dab5', '#a7ffeb',
  '#f6aea9', '#fcad70', '#f9ab00', '#e6c9a8', '#81c995', '#0f9d58',
]

export interface LabelActions {
  onSetColor:       (color: string | null) => void
  onPickCustom:     () => void
  onSetListVis:     (v: LabelListVisibility) => void
  onSetMsgVis:      (v: LabelMsgVisibility) => void
  onRename:         () => void
  onDelete:         () => void
  onAddSubLabel:    () => void
}

/**
 * Sidebar row for a user label. Mirrors the core SidebarNavItem look (same
 * height, radius and inline hover background — module bundles race the host's
 * utilities layer, so `hover:bg-*` cannot be relied on here) and adds Gmail's
 * hover affordance: the unread badge gives way to a ⋮ opening the label menu.
 */
export default function MailLabelItem({
  label, unread, active, to, actions,
}: {
  label:   Label
  unread?: number
  active:  boolean
  to:      string
  actions: LabelActions
}) {
  const { t } = useTranslation('mail')
  const [hovered, setHovered] = useState(false)
  const menu = useMenuDropdown()

  const listVis: LabelListVisibility = label.list_visibility ?? 'show'
  const msgVis:  LabelMsgVisibility  = label.message_list_visibility ?? 'show'
  const color = label.color ?? '#5f6368'

  const ACTIVE_BG = 'var(--color-primary-light, #d3e3fd)'
  const HOVER_BG  = 'color-mix(in srgb, var(--color-primary) 12%, white)'
  const background = active ? ACTIVE_BG : hovered || menu.isOpen ? HOVER_BG : 'transparent'

  // The trailing slot is either the unread badge or the ⋮ — never both, so the
  // row never changes width when the pointer enters it.
  const showMenuButton = hovered || menu.isOpen

  const items: MenuItem[] = [
    {
      type:  'submenu',
      label: t('label_color'),
      icon:  <Tag size={14} style={{ color }} />,
      items: [
        {
          type: 'custom',
          render: close => (
            <div className="px-3 py-2">
              <div className="text-xs text-text-tertiary mb-2">
                {t('label_color')}
              </div>
              <div className="grid grid-cols-6 gap-1.5">
                {LABEL_PALETTE.map(c => (
                  <a
                    key={c}
                    href="#"
                    role="button"
                    aria-label={c}
                    onClick={e => { e.preventDefault(); actions.onSetColor(c); close() }}
                    className="w-6 h-6 rounded-full flex items-center justify-center text-[11px]
                               font-medium cursor-pointer no-underline"
                    style={{ background: c, color: pickReadableInk(c),
                             outline: label.color === c ? '2px solid var(--color-primary)' : 'none',
                             outlineOffset: '1px' }}
                  >
                    a
                  </a>
                ))}
              </div>
            </div>
          ),
        },
        { type: 'separator' },
        {
          type: 'action',
          label: t('label_color_custom'),
          onClick: actions.onPickCustom,
        },
        {
          type: 'action',
          label: t('label_color_remove'),
          disabled: !label.color,
          onClick: () => actions.onSetColor(null),
        },
      ],
    },
    { type: 'separator' },
    { type: 'label', text: t('label_in_label_list') },
    {
      type: 'action', checked: listVis === 'show',
      label: t('label_vis_show'),
      onClick: () => actions.onSetListVis('show'),
    },
    {
      type: 'action', checked: listVis === 'unread',
      label: t('label_vis_unread'),
      onClick: () => actions.onSetListVis('unread'),
    },
    {
      type: 'action', checked: listVis === 'hide',
      label: t('label_vis_hide'),
      onClick: () => actions.onSetListVis('hide'),
    },
    { type: 'separator' },
    { type: 'label', text: t('label_in_message_list') },
    {
      type: 'action', checked: msgVis === 'show',
      label: t('label_vis_show'),
      onClick: () => actions.onSetMsgVis('show'),
    },
    {
      type: 'action', checked: msgVis === 'hide',
      label: t('label_vis_hide'),
      onClick: () => actions.onSetMsgVis('hide'),
    },
    { type: 'separator' },
    {
      type: 'action',
      label: t('common_edit', { defaultValue: 'Modifier' }),
      onClick: actions.onRename,
    },
    {
      type: 'action', danger: true,
      label: t('label_delete'),
      onClick: actions.onDelete,
    },
    {
      type: 'action',
      label: t('label_add_sub'),
      onClick: actions.onAddSubLabel,
    },
  ]

  return (
    <>
      <Link
        to={to}
        aria-label={label.name}
        aria-current={active ? 'page' : undefined}
        onMouseEnter={() => setHovered(true)}
        onMouseLeave={() => setHovered(false)}
        className={`relative flex items-center h-10 gap-3 w-full px-3 rounded-full text-sm text-left
          transition-colors cursor-pointer no-underline outline-none
          focus-visible:ring-2 focus-visible:ring-primary
          ${active ? 'text-primary font-medium' : 'text-text-secondary'}`}
        style={{ backgroundColor: background }}
      >
        <LabelTagIcon color={color} />
        <span className={`truncate flex-1 ${unread ? 'font-medium text-text-primary' : ''}`}>
          {leafName(label.name)}
        </span>

        {showMenuButton ? (
          <a
            href="#"
            role="button"
            aria-label={t('label_options')}
            onClick={e => { e.preventDefault(); e.stopPropagation(); menu.open(e) }}
            className="w-6 h-6 -mr-1 rounded-full flex items-center justify-center flex-shrink-0
                       text-text-secondary cursor-pointer no-underline hover:bg-black/10"
          >
            <MoreVertical size={16} />
          </a>
        ) : unread ? (
          <span className="text-xs bg-primary text-white rounded-full min-w-[18px] h-[18px] px-1
                           flex items-center justify-center flex-shrink-0">
            {unread > 99 ? '99+' : unread}
          </span>
        ) : null}
      </Link>

      {menu.pos && <MenuDropdown items={items} pos={menu.pos} onClose={menu.close} minWidth={220} />}
    </>
  )
}

/** Gmail's filled label tag. */
function LabelTagIcon({ color }: { color: string }) {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill={color} className="flex-shrink-0" aria-hidden="true">
      <path d="M17.63 5.84C17.27 5.33 16.67 5 16 5L5 5.01C3.9 5.01 3 5.9 3 7v10c0 1.1.9 1.99 2 1.99L16 19c.67 0 1.27-.33 1.63-.84L22 12l-4.37-6.16z" />
    </svg>
  )
}

/** "Parent/Child" is displayed as "Child" — the nesting shows in the tree. */
export function leafName(name: string) {
  const i = name.lastIndexOf('/')
  return i < 0 ? name : name.slice(i + 1)
}

/** Black or white ink, whichever reads better on the swatch. */
function pickReadableInk(hex: string) {
  const h = hex.replace('#', '')
  const r = parseInt(h.slice(0, 2), 16)
  const g = parseInt(h.slice(2, 4), 16)
  const b = parseInt(h.slice(4, 6), 16)
  return (r * 299 + g * 587 + b * 114) / 1000 > 150 ? '#202124' : '#ffffff'
}
