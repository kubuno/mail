import { useRef, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { prompt } from '@kubuno/sdk'
import { FontSizeField } from '@ui'
import {
  Bold, Italic, Underline, Palette, Link as LinkIcon, Image as ImageIcon,
  ListOrdered, List, AlignLeft, AlignCenter, AlignRight, Eraser,
} from 'lucide-react'

// Web-safe families for the toolbar (applied via execCommand('fontName')),
// mirroring the compose window so signatures/replies share one visual language.
export const MAIL_FONTS = ['Arial', 'Verdana', 'Trebuchet MS', 'Tahoma', 'Georgia', 'Times New Roman', 'Courier New', 'Comic Sans MS']
export const MAIL_SIZES = [8, 9, 10, 11, 12, 13, 14, 16, 18, 24, 36, 48]
// Text-colour swatches, same palette as the compose toolbar.
export const MAIL_COLORS = ['#202124', '#d93025', '#e8710a', '#188038', '#1a73e8', '#9334e6', '#c2185b', '#5f6368']

function ToolBtn({ onClick, title, children }: {
  onClick: () => void; title: string; children: React.ReactNode
}) {
  return (
    <button
      type="button"
      onMouseDown={e => { e.preventDefault(); onClick() }}
      title={title}
      className="w-7 h-7 flex items-center justify-center rounded hover:bg-black/10 text-text-primary transition-colors flex-shrink-0"
    >
      {children}
    </button>
  )
}

/**
 * Reusable rich-text (WYSIWYG) editor built on the same patterns as the compose
 * window: a `contentEditable` div driven by `document.execCommand`, a formatting
 * toolbar (font/size via `FontSizeField`, bold/italic/underline, text colour,
 * link, image by URL, lists, alignment, clear formatting) and selection
 * restoration so a command lands on the intended text even after the font/size
 * inputs steal focus.
 *
 * ⚠️ The editor is UNCONTROLLED: `value` seeds `innerHTML` ONCE on mount, then
 * changes are read back through `onChange` on input. Never re-write `innerHTML`
 * on every keystroke — it would collapse the caret. To reset the content
 * externally, remount the editor with a new React `key`.
 */
export function RichTextEditor({ value, onChange, placeholder, minHeight = 120 }: {
  value:        string
  onChange:     (html: string) => void
  placeholder?: string
  minHeight?:   number
}) {
  const { t } = useTranslation('mail')
  const bodyRef = useRef<HTMLDivElement>(null)
  const [fontName, setFontName] = useState('Arial')
  const [fontSz,   setFontSz]   = useState('13')
  const [colorOpen, setColorOpen] = useState(false)

  // Seed the editable content once — see the class doc about staying uncontrolled.
  useEffect(() => {
    if (bodyRef.current) bodyRef.current.innerHTML = value ?? ''
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // Track the last selection INSIDE this editor. The font/size selectors are
  // editable inputs that steal focus and collapse the body selection, so we
  // restore it before applying a command.
  const savedRangeRef = useRef<Range | null>(null)
  useEffect(() => {
    const save = () => {
      const sel = document.getSelection()
      if (sel && sel.rangeCount && bodyRef.current?.contains(sel.getRangeAt(0).commonAncestorContainer)) {
        savedRangeRef.current = sel.getRangeAt(0).cloneRange()
      }
    }
    document.addEventListener('selectionchange', save)
    return () => document.removeEventListener('selectionchange', save)
  }, [])
  const restoreSel = () => {
    bodyRef.current?.focus()
    const r = savedRangeRef.current
    const sel = document.getSelection()
    if (r && sel && bodyRef.current?.contains(r.commonAncestorContainer)) {
      sel.removeAllRanges(); sel.addRange(r)
    }
  }

  const emitChange = () => onChange(bodyRef.current?.innerHTML ?? '')

  const exec = (cmd: string, val?: string) => {
    restoreSel()
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    ;(document as any).execCommand(cmd, false, val ?? undefined)
    emitChange()
  }

  // execCommand('fontSize') only accepts the legacy 1-7 scale, so we tag the
  // selection with size 7 then rewrite those <font> markers to the real px value
  // (the same reliable trick as the compose window).
  const applyFontSizePx = (px: string) => {
    const ed = bodyRef.current
    if (!ed) return
    restoreSel()
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const cmd = (c: string, v?: string) => (document as any).execCommand(c, false, v)
    cmd('styleWithCSS', 'false')
    cmd('fontSize', '7')
    ed.querySelectorAll('font[size="7"]').forEach(el => {
      const e = el as HTMLElement
      e.removeAttribute('size')
      e.style.fontSize = `${px}px`
    })
    emitChange()
  }

  const insertLink = async () => {
    const url = await prompt({ title: t('mail_insert_link', { defaultValue: 'Insérer un lien' }), placeholder: 'https://…' })
    if (url?.trim()) exec('createLink', url.trim())
  }
  const insertImage = async () => {
    const url = await prompt({ title: t('mail_insert_image_url', { defaultValue: "Adresse de l'image" }), placeholder: 'https://…' })
    if (url?.trim()) exec('insertImage', url.trim())
  }

  return (
    <div className="rounded-lg border border-border bg-surface-0 overflow-hidden">
      {/* ── Toolbar ─────────────────────────────────────────────────────── */}
      <div className="flex items-center flex-wrap gap-0.5 px-2 py-1.5 bg-surface-1 border-b border-border">
        <FontSizeField
          font={fontName} onFontChange={v => { setFontName(v); bodyRef.current?.focus(); exec('fontName', v) }} fonts={MAIL_FONTS}
          size={fontSz} onSizeChange={v => { setFontSz(v); applyFontSizePx(v) }} sizes={MAIL_SIZES}
          minSize={6} maxSize={96} height={26} fontWidth={118} sizeWidth={58} fontSize={14}
        />
        <div className="w-px h-4 bg-border mx-0.5" />
        <ToolBtn onClick={() => exec('bold')}      title={t('mail_bold', { defaultValue: 'Gras' })}><Bold size={13} /></ToolBtn>
        <ToolBtn onClick={() => exec('italic')}    title={t('mail_italic', { defaultValue: 'Italique' })}><Italic size={13} /></ToolBtn>
        <ToolBtn onClick={() => exec('underline')} title={t('mail_underline', { defaultValue: 'Souligné' })}><Underline size={13} /></ToolBtn>
        {/* Text colour popover */}
        <div className="relative">
          <ToolBtn onClick={() => setColorOpen(v => !v)} title={t('mail_text_color', { defaultValue: 'Couleur du texte' })}><Palette size={13} /></ToolBtn>
          {colorOpen && (
            <>
              <div className="fixed inset-0 z-40" onClick={() => setColorOpen(false)} />
              <div className="absolute top-full mt-1 left-0 z-50 bg-white border border-border rounded-lg shadow-lg p-2 grid grid-cols-4 gap-1.5 w-40">
                {MAIL_COLORS.map(c => (
                  <button
                    key={c} type="button"
                    onMouseDown={e => { e.preventDefault(); exec('foreColor', c); setColorOpen(false) }}
                    className="w-7 h-7 rounded-full border border-border" style={{ background: c }} title={c}
                  />
                ))}
              </div>
            </>
          )}
        </div>
        <div className="w-px h-4 bg-border mx-0.5" />
        <ToolBtn onClick={insertLink}  title={t('mail_insert_link', { defaultValue: 'Insérer un lien' })}><LinkIcon size={13} /></ToolBtn>
        <ToolBtn onClick={insertImage} title={t('mail_insert_image', { defaultValue: 'Insérer une image' })}><ImageIcon size={13} /></ToolBtn>
        <div className="w-px h-4 bg-border mx-0.5" />
        <ToolBtn onClick={() => exec('insertOrderedList')}   title={t('mail_ordered_list', { defaultValue: 'Liste numérotée' })}><ListOrdered size={13} /></ToolBtn>
        <ToolBtn onClick={() => exec('insertUnorderedList')} title={t('mail_bullet_list', { defaultValue: 'Liste à puces' })}><List size={13} /></ToolBtn>
        <div className="w-px h-4 bg-border mx-0.5" />
        <ToolBtn onClick={() => exec('justifyLeft')}   title={t('mail_align_left', { defaultValue: 'Aligner à gauche' })}><AlignLeft size={13} /></ToolBtn>
        <ToolBtn onClick={() => exec('justifyCenter')} title={t('mail_align_center', { defaultValue: 'Centrer' })}><AlignCenter size={13} /></ToolBtn>
        <ToolBtn onClick={() => exec('justifyRight')}  title={t('mail_align_right', { defaultValue: 'Aligner à droite' })}><AlignRight size={13} /></ToolBtn>
        <div className="w-px h-4 bg-border mx-0.5" />
        <ToolBtn onClick={() => exec('removeFormat')} title={t('mail_clear_format', { defaultValue: 'Effacer la mise en forme' })}><Eraser size={13} /></ToolBtn>
      </div>

      {/* ── Editable body ───────────────────────────────────────────────── */}
      <div
        ref={bodyRef}
        contentEditable
        suppressContentEditableWarning
        onInput={emitChange}
        data-placeholder={placeholder ?? ''}
        className="px-3 py-2 text-sm text-text-primary outline-none overflow-y-auto overflow-x-auto break-words
                   [&_img]:max-w-full [&_img]:h-auto
                   empty:before:content-[attr(data-placeholder)] empty:before:text-text-tertiary"
        style={{ minHeight, lineHeight: '1.6' }}
      />
    </div>
  )
}
