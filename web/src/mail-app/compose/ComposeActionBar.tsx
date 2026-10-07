import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { MenuDropdown, type MenuItem, type MenuDropdownPos } from '@ui'
import { Paperclip, Link, Smile, Image, Lock, Palette, MoreHorizontal, Trash2, ShieldCheck, PenLine } from 'lucide-react'
import { EMOJIS, COLORS, IconBtn } from './parts'
import SendButton from './SendButton'

// Bottom action row of the composer: the « Envoyer » split button, the format
// toggle, text colour, attach/link/emoji/image, confidential mode, the PGP
// shield, the signature pen, the « ⋯ » menu, the autosave status and discard.
// Extracted verbatim from ComposeWindow — classes, positioning and behaviour
// are unchanged. The colour/emoji popovers and the PGP/signature/more menus own
// their open state locally; their menu ITEMS are built by the parent (they
// depend on the whole compose state) and passed in.
export default function ComposeActionBar({
  onSendNow, onSchedule, sendDisabled, sendPending, scheduleAutoOpen,
  plainText, showFmt, setShowFmt,
  exec, fileRef, onPickFiles, insertLink, insertImageUrl,
  confidential, setConfidential,
  gpgEnabled, pgpSign, pgpEncrypt, secItems,
  sigItems, moreItems,
  draftStatus, onDiscard,
}: {
  onSendNow: () => void
  onSchedule: (iso: string) => void
  sendDisabled: boolean
  sendPending: boolean
  scheduleAutoOpen: boolean
  plainText: boolean
  showFmt: boolean
  setShowFmt: React.Dispatch<React.SetStateAction<boolean>>
  exec: (cmd: string, value?: string) => void
  fileRef: React.RefObject<HTMLInputElement | null>
  onPickFiles: (files: FileList | null) => void
  insertLink: () => void
  insertImageUrl: () => void
  confidential: boolean
  setConfidential: React.Dispatch<React.SetStateAction<boolean>>
  gpgEnabled: boolean
  pgpSign: boolean
  pgpEncrypt: boolean
  secItems: MenuItem[]
  sigItems: MenuItem[]
  moreItems: MenuItem[]
  draftStatus: string
  onDiscard: () => void
}) {
  const { t } = useTranslation('mail')
  const [emojiOpen, setEmojiOpen] = useState(false)
  const [colorOpen, setColorOpen] = useState(false)
  const [secMenu,  setSecMenu]  = useState<MenuDropdownPos | null>(null)
  const [sigMenu,  setSigMenu]  = useState<MenuDropdownPos | null>(null)
  const [moreMenu, setMoreMenu] = useState<MenuDropdownPos | null>(null)

  return (
    <>
      <div className="flex items-center gap-1 px-4 py-3 border-t border-border flex-shrink-0">
        {/* Envoyer + programmer */}
        <SendButton
          onSendNow={onSendNow}
          onSchedule={onSchedule}
          disabled={sendDisabled}
          pending={sendPending}
          autoOpen={scheduleAutoOpen}
        />

        {/* Aa toggle — hidden in plain-text mode (no formatting to reveal) */}
        {!plainText && (
          <button
            onClick={() => setShowFmt(v => !v)}
            className={`w-9 h-9 flex items-center justify-center rounded-full text-sm font-semibold transition-colors flex-shrink-0 ${
              showFmt ? 'bg-primary/10 text-primary' : 'bg-surface-2 text-text-secondary hover:bg-surface-3'
            }`}
            title={t('mail_formatting')}
          >
            Aa
          </button>
        )}

        {/* Couleur du texte */}
        <div className="relative">
          <IconBtn title={t('mail_text_color', { defaultValue: 'Couleur du texte' })} onClick={() => setColorOpen(v => !v)}><Palette size={15} /></IconBtn>
          {colorOpen && (
            <>
              <div className="fixed inset-0 z-40" onClick={() => setColorOpen(false)} />
              <div className="absolute bottom-full mb-1 left-0 z-50 bg-white border border-border rounded-lg shadow-lg p-2 grid grid-cols-4 gap-1.5 w-40">
                {COLORS.map(c => (
                  <button key={c} onMouseDown={e => { e.preventDefault(); exec('foreColor', c); setColorOpen(false) }}
                    className="w-7 h-7 rounded-full border border-border" style={{ background: c }} title={c} />
                ))}
              </div>
            </>
          )}
        </div>

        <IconBtn title={t('mail_attach_file', { defaultValue: 'Joindre des fichiers' })} onClick={() => fileRef.current?.click()}><Paperclip size={15} /></IconBtn>
        <input ref={fileRef} type="file" multiple hidden onChange={e => onPickFiles(e.target.files)} />

        <IconBtn title={t('mail_insert_link', { defaultValue: 'Insérer un lien' })} onClick={insertLink}><Link size={15} /></IconBtn>

        {/* Emoji */}
        <div className="relative">
          <IconBtn title={t('mail_insert_emoji', { defaultValue: 'Emoji' })} onClick={() => setEmojiOpen(v => !v)}><Smile size={15} /></IconBtn>
          {emojiOpen && (
            <>
              <div className="fixed inset-0 z-40" onClick={() => setEmojiOpen(false)} />
              <div className="absolute bottom-full mb-1 left-0 z-50 bg-white border border-border rounded-lg shadow-lg p-2 grid grid-cols-6 gap-1 w-56">
                {EMOJIS.map(em => (
                  <button key={em} onMouseDown={e => { e.preventDefault(); exec('insertText', em); setEmojiOpen(false) }}
                    className="w-7 h-7 flex items-center justify-center rounded hover:bg-surface-2 text-lg">{em}</button>
                ))}
              </div>
            </>
          )}
        </div>

        <IconBtn title={t('mail_insert_image', { defaultValue: 'Insérer une image' })} onClick={insertImageUrl}><Image size={15} /></IconBtn>
        <IconBtn title={t('mail_confidential', { defaultValue: 'Mode confidentiel' })} onClick={() => setConfidential(v => !v)}>
          <Lock size={15} className={confidential ? 'text-primary' : ''} />
        </IconBtn>
        {gpgEnabled && (
          <IconBtn
            title={t('mail_pgp_security', { defaultValue: 'Signer / Chiffrer (OpenPGP)' })}
            onClick={e => { const r = (e!.currentTarget as HTMLElement).getBoundingClientRect(); setSecMenu(p => (p ? null : { top: r.top, left: r.left })) }}
          >
            <ShieldCheck size={15} className={(pgpSign || pgpEncrypt) ? 'text-primary' : ''} />
          </IconBtn>
        )}
        <IconBtn
          title={t('mail_signature', { defaultValue: 'Insérer une signature' })}
          onClick={e => { const r = (e!.currentTarget as HTMLElement).getBoundingClientRect(); setSigMenu(p => (p ? null : { top: r.top, left: r.left })) }}
        >
          <PenLine size={15} />
        </IconBtn>
        <IconBtn title={t('more_options')} onClick={e => { const r = (e!.currentTarget as HTMLElement).getBoundingClientRect(); setMoreMenu({ top: r.top, left: r.left }) }}><MoreHorizontal size={15} /></IconBtn>

        <div className="flex-1" />

        {/* Auto-save status, à la Gmail. */}
        {draftStatus !== 'idle' && (
          <span className="text-[14px] text-text-tertiary mr-1 select-none">
            {draftStatus === 'saving'
              ? t('mail_draft_saving', { defaultValue: 'Enregistrement…' })
              : t('mail_draft_saved',  { defaultValue: 'Brouillon enregistré' })}
          </span>
        )}

        <button
          onClick={onDiscard}
          className="p-2 rounded-full hover:bg-danger/10 hover:text-danger text-text-tertiary transition-colors"
          title={t('discard')}
        >
          <Trash2 size={15} />
        </button>
      </div>

      {moreMenu && <MenuDropdown items={moreItems} pos={moreMenu} onClose={() => setMoreMenu(null)} />}
      {secMenu && <MenuDropdown items={secItems} pos={{ ...secMenu, minWidth: 220 }} onClose={() => setSecMenu(null)} />}
      {sigMenu && <MenuDropdown items={sigItems} pos={{ ...sigMenu, minWidth: 200 }} onClose={() => setSigMenu(null)} />}
    </>
  )
}
