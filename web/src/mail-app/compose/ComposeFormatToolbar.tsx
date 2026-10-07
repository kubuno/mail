import { useTranslation } from 'react-i18next'
import { FontSizeField, type MenuDropdownPos } from '@ui'
import {
  Undo2, Redo2, Bold, Italic, Underline, Strikethrough,
  AlignLeft, ListOrdered, List, Indent, Outdent, ChevronDown,
} from 'lucide-react'
import { MAIL_FONTS, MAIL_SIZES, ToolBtn } from './parts'

// The rich-text formatting toolbar (undo/redo, font family/size, B/I/U/S,
// alignment dropdown, lists, indent). Extracted verbatim from ComposeWindow —
// behaviour, classes and positioning are unchanged.
export default function ComposeFormatToolbar({
  exec, fontName, setFontName, fontSz, setFontSz, applyFontSizePx, focusBody, setAlignMenu,
}: {
  exec: (cmd: string, value?: string) => void
  fontName: string
  setFontName: (v: string) => void
  fontSz: string
  setFontSz: (v: string) => void
  applyFontSizePx: (px: string) => void
  focusBody: () => void
  setAlignMenu: (updater: (p: MenuDropdownPos | null) => MenuDropdownPos | null) => void
}) {
  const { t } = useTranslation('mail')
  return (
    <div className="flex items-center flex-wrap gap-0.5 px-3 py-1.5 mx-3 mb-2 bg-surface-1 rounded-lg border border-border flex-shrink-0">
      <ToolBtn onClick={() => exec('undo')}   title={t('common_undo')}><Undo2 size={13} /></ToolBtn>
      <ToolBtn onClick={() => exec('redo')}   title={t('common_redo')}><Redo2 size={13} /></ToolBtn>
      <div className="w-px h-4 bg-border mx-0.5" />
      <FontSizeField
        font={fontName} onFontChange={v => { setFontName(v); focusBody(); exec('fontName', v) }} fonts={MAIL_FONTS}
        size={fontSz} onSizeChange={v => { setFontSz(v); applyFontSizePx(v) }} sizes={MAIL_SIZES}
        minSize={6} maxSize={96} height={26} fontWidth={118} sizeWidth={58} fontSize={14}
      />
      <div className="w-px h-4 bg-border mx-0.5" />
      <ToolBtn onClick={() => exec('bold')}          title={t('mail_bold')}><Bold size={13} /></ToolBtn>
      <ToolBtn onClick={() => exec('italic')}        title={t('mail_italic')}><Italic size={13} /></ToolBtn>
      <ToolBtn onClick={() => exec('underline')}     title={t('mail_underline')}><Underline size={13} /></ToolBtn>
      <ToolBtn onClick={() => exec('strikeThrough')} title={t('mail_strikethrough')}><Strikethrough size={13} /></ToolBtn>
      <div className="w-px h-4 bg-border mx-0.5" />
      <button
        onMouseDown={e => {
          e.preventDefault()
          // Capture the rect BEFORE the state updater — e.currentTarget is null
          // once React runs the updater callback.
          const r = e.currentTarget.getBoundingClientRect()
          setAlignMenu(p => (p ? null : { top: r.bottom + 4, left: r.left }))
        }}
        title={t('mail_align', { defaultValue: 'Alignement' })}
        className="h-7 px-1.5 flex items-center gap-0.5 rounded hover:bg-black/10 text-text-primary transition-colors flex-shrink-0"
      >
        <AlignLeft size={13} /><ChevronDown size={11} />
      </button>
      <div className="w-px h-4 bg-border mx-0.5" />
      <ToolBtn onClick={() => exec('insertOrderedList')}   title={t('mail_ordered_list')}><ListOrdered size={13} /></ToolBtn>
      <ToolBtn onClick={() => exec('insertUnorderedList')} title={t('mail_bullet_list')}><List size={13} /></ToolBtn>
      <ToolBtn onClick={() => exec('indent')}  title={t('mail_indent')}><Indent size={13} /></ToolBtn>
      <ToolBtn onClick={() => exec('outdent')} title={t('mail_outdent')}><Outdent size={13} /></ToolBtn>
    </div>
  )
}
