import { useState, useRef, useEffect } from 'react'
import { useTranslation } from 'react-i18next'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { FloatingWindow, Dropdown, MenuDropdown, type MenuItem, type MenuDropdownPos } from '@ui'
import { prompt } from '@kubuno/sdk'
import { useUndoSendStore } from './undoSendStore'
import {
  X, Minus, Paperclip, Link, Smile, Image, Lock,
  Undo2, Redo2, Bold, Italic, Underline, Strikethrough,
  AlignLeft, AlignCenter, AlignRight, ListOrdered, List, Indent, Outdent,
  ChevronDown, Type, Palette, MoreHorizontal, Trash2, Eraser,
} from 'lucide-react'
import { mailApi, EmailAddress } from './api'
import { useMailStore } from './store'

const MIN_W = 420, MIN_H = 320

function ToolBtn({ onClick, title, children }: {
  onClick: () => void; title: string; children: React.ReactNode
}) {
  return (
    <button
      onMouseDown={e => { e.preventDefault(); onClick() }}
      title={title}
      className="w-7 h-7 flex items-center justify-center rounded hover:bg-black/10 text-text-secondary transition-colors flex-shrink-0"
    >
      {children}
    </button>
  )
}

function IconBtn({ onClick, title, children }: {
  onClick?: (e?: React.MouseEvent) => void; title: string; children: React.ReactNode
}) {
  return (
    <button
      onClick={onClick}
      title={title}
      className="p-1.5 rounded-full hover:bg-surface-2 text-text-tertiary transition-colors flex-shrink-0"
    >
      {children}
    </button>
  )
}

export default function ComposeWindow() {
  const { t } = useTranslation('mail')
  const { setComposeOpen, accounts, composeInitial, setComposeInitial } = useMailStore()
  const scheduleUndo = useUndoSendStore(s => s.schedule)
  const qc = useQueryClient()

  const defaultAccount = accounts.find(a => a.is_default) ?? accounts[0]
  const accountId = defaultAccount?.id ?? ''

  const [to,        setTo]        = useState<EmailAddress[]>(composeInitial?.to ?? [])
  const [toInput,   setToInput]   = useState('')
  const [cc,        setCc]        = useState<EmailAddress[]>(composeInitial?.cc ?? [])
  const [ccInput,   setCcInput]   = useState('')
  const [showCc,    setShowCc]    = useState(!!composeInitial?.cc.length)
  const [subject,   setSubject]   = useState(composeInitial?.subject ?? '')
  const [showFmt,   setShowFmt]   = useState(true)
  const [minimized, setMinimized] = useState(false)
  const [fontName,  setFontName]  = useState('sans-serif')
  const [fontSz,    setFontSz]    = useState('3')

  const bodyRef = useRef<HTMLDivElement>(null)

  // Pré-remplissage (ré-ouverture après « Annuler l'envoi »).
  useEffect(() => {
    if (composeInitial) {
      const html = composeInitial.bodyHtml
      requestAnimationFrame(() => { if (bodyRef.current) bodyRef.current.innerHTML = html })
      setComposeInitial(null)
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // Envoi immédiat → différé de 5 s avec possibilité d'annuler (toast dans MailApp).
  const sendNow = () => {
    if (!to.length || !accountId) return
    let body = bodyRef.current?.innerHTML ?? ''
    if (confidential) {
      body += `<br><br><div style="border-top:1px solid #dadce0;color:#5f6368;font-size:12px;padding-top:6px">🔒 ${t('mail_confidential_note', { defaultValue: 'Ce message est confidentiel.' })}</div>`
    }
    const payload = {
      account_id:   accountId,
      to_addresses: to,
      cc_addresses: cc.length ? cc : undefined,
      subject,
      body_html:    body,
      attachments:  attachments.length ? attachments.map(a => ({ filename: a.filename, mime: a.mime, content: a.content })) : undefined,
    }
    scheduleUndo(payload, () => {
      mailApi.sendMail(payload)
        .then(() => { qc.invalidateQueries({ queryKey: ['mail-threads'] }); qc.invalidateQueries({ queryKey: ['mail-counts'] }) })
        .catch(() => {})
    })
    setComposeOpen(false)
  }

  // ── Address helpers ─────────────────────────────────────────────────────────
  const addTo = () => {
    const v = toInput.trim()
    if (v && v.includes('@')) { setTo(p => [...p, { email: v }]); setToInput('') }
  }
  const addCc = () => {
    const v = ccInput.trim()
    if (v && v.includes('@')) { setCc(p => [...p, { email: v }]); setCcInput('') }
  }

  // ── execCommand ─────────────────────────────────────────────────────────────
  const exec = (cmd: string, value?: string) => {
    bodyRef.current?.focus()
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    ;(document as any).execCommand(cmd, false, value ?? undefined)
  }

  // ── Pièces jointes ────────────────────────────────────────────────────────────
  const fileRef = useRef<HTMLInputElement>(null)
  const [attachments, setAttachments] = useState<{ filename: string; mime: string; content: string; size: number }[]>([])
  const onPickFiles = async (files: FileList | null) => {
    if (!files?.length) return
    const read = (f: File) => new Promise<{ filename: string; mime: string; content: string; size: number }>(res => {
      const r = new FileReader()
      r.onload = () => res({ filename: f.name, mime: f.type || 'application/octet-stream', content: String(r.result).split(',')[1] ?? '', size: f.size })
      r.readAsDataURL(f)
    })
    const items = await Promise.all([...files].map(read))
    setAttachments(prev => [...prev, ...items])
    if (fileRef.current) fileRef.current.value = ''
  }
  const fmtSize = (n: number) => n < 1024 ? `${n} o` : n < 1048576 ? `${Math.round(n / 1024)} Ko` : `${(n / 1048576).toFixed(1)} Mo`

  // ── Boutons de la barre d'action ───────────────────────────────────────────────
  const insertLink = async () => {
    const url = await prompt({ title: t('mail_insert_link', { defaultValue: 'Insérer un lien' }), placeholder: 'https://…' })
    if (url?.trim()) exec('createLink', url.trim())
  }
  const insertImageUrl = async () => {
    const url = await prompt({ title: t('mail_insert_image', { defaultValue: 'Insérer une image' }), placeholder: 'https://…/image.png' })
    if (url?.trim()) exec('insertImage', url.trim())
  }
  const insertSignature = () => {
    const sig = defaultAccount?.name || defaultAccount?.email_address || ''
    exec('insertHTML', `<br><br><div style="color:#5f6368">--<br>${sig}</div>`)
  }
  const [emojiOpen,    setEmojiOpen]    = useState(false)
  const [colorOpen,    setColorOpen]    = useState(false)
  const [moreMenu,     setMoreMenu]     = useState<MenuDropdownPos | null>(null)
  const [confidential, setConfidential] = useState(false)
  const EMOJIS = ['😀','😅','😉','😍','😘','😎','🤔','🙏','👍','👎','👏','🙌','🎉','🔥','✅','❌','⭐','❤️','💡','📎','📅','⏰']
  const COLORS = ['#202124','#d93025','#e8710a','#188038','#1a73e8','#9334e6','#c2185b','#5f6368']
  const moreItems: MenuItem[] = [
    { type: 'action', label: t('mail_clear_format', { defaultValue: 'Effacer la mise en forme' }), icon: <Eraser size={15} />, onClick: () => exec('removeFormat') },
    { type: 'action', label: t('mail_print', { defaultValue: 'Imprimer' }), onClick: () => { const w = window.open('', '_blank'); if (w) { w.document.write(bodyRef.current?.innerHTML ?? ''); w.document.close(); w.print() } } },
  ]

  // ── Send ────────────────────────────────────────────────────────────────────
  const sendMut = useMutation({
    mutationFn: (scheduledAt?: string) => mailApi.sendMail({
      account_id:   accountId,
      to_addresses: to,
      cc_addresses: cc.length ? cc : undefined,
      subject,
      body_html:    bodyRef.current?.innerHTML ?? '',
      scheduled_at: scheduledAt,
    }),
    onSuccess: () => {
      setComposeOpen(false)
      qc.invalidateQueries({ queryKey: ['mail-threads'] })
      qc.invalidateQueries({ queryKey: ['mail-scheduled'] })
      qc.invalidateQueries({ queryKey: ['mail-counts'] })
    },
  })

  const [sendMenuPos, setSendMenuPos] = useState<MenuDropdownPos | null>(null)
  const schedulePresets = () => {
    const now = new Date()
    const later   = new Date(now); later.setHours(now.getHours() + 2, 0, 0, 0)
    const tom     = new Date(now); tom.setDate(now.getDate() + 1); tom.setHours(8, 0, 0, 0)
    const mon     = new Date(now); mon.setDate(now.getDate() + ((8 - now.getDay()) % 7 || 7)); mon.setHours(8, 0, 0, 0)
    return [
      { label: t('schedule_later',    { defaultValue: 'Plus tard (2 h)' }),     at: later.toISOString() },
      { label: t('schedule_tomorrow', { defaultValue: 'Demain matin' }),         at: tom.toISOString() },
      { label: t('schedule_monday',   { defaultValue: 'Lundi matin' }),          at: mon.toISOString() },
    ]
  }

  // ── Minimized bar ───────────────────────────────────────────────────────────
  if (minimized) {
    return (
      <div
        className="fixed bottom-0 right-4 w-72 bg-[#404040] rounded-t-xl shadow-xl z-50 flex items-center justify-between px-4 py-2.5 cursor-pointer"
        onClick={() => setMinimized(false)}
      >
        <span className="text-sm text-white font-medium truncate">{subject || t('new_message')}</span>
        <div className="flex items-center gap-2">
          <button onMouseDown={e => e.stopPropagation()} onClick={e => { e.stopPropagation(); setMinimized(false) }} className="text-white/70 hover:text-white"><Minus size={13} /></button>
          <button onMouseDown={e => e.stopPropagation()} onClick={e => { e.stopPropagation(); setComposeOpen(false) }} className="text-white/70 hover:text-white"><X size={13} /></button>
        </div>
      </div>
    )
  }

  return (
    <FloatingWindow
      title={t('new_message')}
      onClose={() => setComposeOpen(false)}
      defaultWidth={600}
      defaultHeight={520}
      minWidth={MIN_W}
      minHeight={MIN_H}
      resizable
      titleActions={
        <button
          onClick={() => setMinimized(true)}
          className="p-1.5 rounded-lg text-text-tertiary hover:text-text-primary hover:bg-surface-2 transition-colors"
          title={t('mail_less', { defaultValue: 'Réduire' })}
        >
          <Minus size={15} />
        </button>
      }
    >
      {/* ── Destinataires ────────────────────────────────────────────────────── */}
      <div className="flex items-start gap-2 px-4 py-2.5 border-b border-border flex-shrink-0">
        <div className="flex-1 flex flex-wrap gap-1 items-center min-w-0">
          {to.map((a, i) => (
            <span key={i} className="flex items-center gap-1 bg-surface-2 text-text-secondary text-xs px-2 py-0.5 rounded-full">
              {a.name ? `${a.name} <${a.email}>` : a.email}
              <button onClick={() => setTo(p => p.filter((_, idx) => idx !== i))}><X size={9} /></button>
            </span>
          ))}
          <input
            type="email"
            value={toInput}
            onChange={e => setToInput(e.target.value)}
            onKeyDown={e => { if (e.key === 'Enter' || e.key === ',') { e.preventDefault(); addTo() } }}
            onBlur={addTo}
            placeholder={to.length ? '' : t('mail_add_recipient')}
            className="flex-1 min-w-32 text-sm outline-none bg-transparent text-text-primary placeholder:text-text-tertiary"
          />
        </div>
        {!showCc && (
          <button onClick={() => setShowCc(true)} className="text-xs text-text-tertiary hover:text-primary whitespace-nowrap flex-shrink-0 mt-0.5">
            CC
          </button>
        )}
      </div>

      {/* ── CC ───────────────────────────────────────────────────────────────── */}
      {showCc && (
        <div className="flex items-start px-4 py-2 border-b border-border flex-shrink-0">
          <span className="text-sm text-text-tertiary mr-3 mt-0.5 flex-shrink-0">CC</span>
          <div className="flex-1 flex flex-wrap gap-1 items-center">
            {cc.map((a, i) => (
              <span key={i} className="flex items-center gap-1 bg-surface-2 text-xs px-2 py-0.5 rounded-full text-text-secondary">
                {a.email}
                <button onClick={() => setCc(p => p.filter((_, idx) => idx !== i))}><X size={9} /></button>
              </span>
            ))}
            <input
              type="email"
              value={ccInput}
              onChange={e => setCcInput(e.target.value)}
              onKeyDown={e => { if (e.key === 'Enter' || e.key === ',') { e.preventDefault(); addCc() } }}
              onBlur={addCc}
              placeholder={t('to_add')}
              className="flex-1 min-w-24 text-sm outline-none bg-transparent text-text-primary placeholder:text-text-tertiary"
            />
          </div>
        </div>
      )}

      {/* ── Objet ────────────────────────────────────────────────────────────── */}
      <div className="px-4 py-2.5 border-b border-border flex-shrink-0">
        <input
          type="text"
          value={subject}
          onChange={e => setSubject(e.target.value)}
          placeholder={t('subject')}
          className="w-full text-sm outline-none bg-transparent text-text-primary placeholder:text-text-tertiary"
        />
      </div>

      {/* ── Body (contenteditable) ────────────────────────────────────────────── */}
      <div
        ref={bodyRef}
        contentEditable
        suppressContentEditableWarning
        data-placeholder={t('body')}
        className="flex-1 px-4 py-3 text-sm text-text-primary outline-none overflow-y-auto empty:before:content-[attr(data-placeholder)] empty:before:text-text-tertiary"
        style={{ lineHeight: '1.6' }}
      />

      {/* ── Pièces jointes ────────────────────────────────────────────────────── */}
      {attachments.length > 0 && (
        <div className="flex flex-wrap gap-2 px-4 py-2 border-t border-border flex-shrink-0">
          {attachments.map((a, i) => (
            <span key={i} className="flex items-center gap-2 bg-surface-1 border border-border rounded-lg px-2.5 py-1 text-xs text-text-secondary">
              <Paperclip size={12} className="text-text-tertiary" />
              <span className="max-w-[180px] truncate">{a.filename}</span>
              <span className="text-text-tertiary">{fmtSize(a.size)}</span>
              <button onClick={() => setAttachments(p => p.filter((_, idx) => idx !== i))} className="hover:text-danger"><X size={11} /></button>
            </span>
          ))}
        </div>
      )}

      {/* ── Format toolbar ───────────────────────────────────────────────────── */}
      {showFmt && (
        <div className="flex items-center flex-wrap gap-0.5 px-3 py-1.5 mx-3 mb-2 bg-surface-1 rounded-full border border-border flex-shrink-0">
          <ToolBtn onClick={() => exec('undo')}   title={t('common_undo')}><Undo2 size={13} /></ToolBtn>
          <ToolBtn onClick={() => exec('redo')}   title={t('common_redo')}><Redo2 size={13} /></ToolBtn>
          <div className="w-px h-4 bg-border mx-0.5" />
          <Dropdown
            value={fontName}
            onChange={v => { setFontName(v); bodyRef.current?.focus(); exec('fontName', v) }}
            options={[
              { value: 'sans-serif', label: t('mail_font_sans') },
              { value: 'serif',      label: t('mail_font_serif') },
              { value: 'Georgia',    label: 'Georgia' },
              { value: 'Arial',      label: 'Arial' },
              { value: 'monospace',  label: t('mail_font_mono') },
            ]}
            height={26} fontSize={12} width={118}
          />
          <Dropdown
            value={fontSz}
            onChange={v => { setFontSz(v); bodyRef.current?.focus(); exec('fontSize', v) }}
            options={['8','10','12','14','18','24','36'].map((sz, i) => ({ value: String(i + 1), label: sz }))}
            height={26} fontSize={12} width={64}
          />
          <div className="w-px h-4 bg-border mx-0.5" />
          <ToolBtn onClick={() => exec('bold')}          title={t('mail_bold')}><Bold size={13} /></ToolBtn>
          <ToolBtn onClick={() => exec('italic')}        title={t('mail_italic')}><Italic size={13} /></ToolBtn>
          <ToolBtn onClick={() => exec('underline')}     title={t('mail_underline')}><Underline size={13} /></ToolBtn>
          <ToolBtn onClick={() => exec('strikeThrough')} title={t('mail_strikethrough')}><Strikethrough size={13} /></ToolBtn>
          <div className="w-px h-4 bg-border mx-0.5" />
          <ToolBtn onClick={() => exec('justifyLeft')}   title={t('mail_align_left')}><AlignLeft size={13} /></ToolBtn>
          <ToolBtn onClick={() => exec('justifyCenter')} title={t('mail_align_center')}><AlignCenter size={13} /></ToolBtn>
          <ToolBtn onClick={() => exec('justifyRight')}  title={t('mail_align_right')}><AlignRight size={13} /></ToolBtn>
          <div className="w-px h-4 bg-border mx-0.5" />
          <ToolBtn onClick={() => exec('insertOrderedList')}   title={t('mail_ordered_list')}><ListOrdered size={13} /></ToolBtn>
          <ToolBtn onClick={() => exec('insertUnorderedList')} title={t('mail_bullet_list')}><List size={13} /></ToolBtn>
          <ToolBtn onClick={() => exec('indent')}  title={t('mail_indent')}><Indent size={13} /></ToolBtn>
          <ToolBtn onClick={() => exec('outdent')} title={t('mail_outdent')}><Outdent size={13} /></ToolBtn>
        </div>
      )}

      {/* ── Action bar ───────────────────────────────────────────────────────── */}
      <div className="flex items-center gap-1 px-4 py-3 border-t border-border flex-shrink-0">
        {/* Envoyer + programmer */}
        <div className="relative flex items-stretch flex-shrink-0 mr-2">
          <button
            onClick={sendNow}
            disabled={!to.length || !accountId}
            className="flex items-center gap-2 h-9 pl-5 pr-4 text-sm font-medium bg-primary text-white rounded-l-full hover:bg-primary-hover disabled:opacity-50 transition-colors"
          >
            {t('mail_send')}
          </button>
          <button
            onClick={e => { const r = e.currentTarget.getBoundingClientRect(); setSendMenuPos(p => p ? null : { top: r.top - 4, left: r.left - 200 }) }}
            disabled={sendMut.isPending || !to.length || !accountId}
            title={t('schedule_send', { defaultValue: 'Programmer l\'envoi' })}
            className="flex items-center h-9 px-1.5 bg-primary text-white rounded-r-full border-l border-white/25 hover:bg-primary-hover disabled:opacity-50 transition-colors ml-px"
          >
            <ChevronDown size={14} />
          </button>
          {sendMenuPos && (
            <MenuDropdown
              pos={{ ...sendMenuPos, minWidth: 224 }}
              onClose={() => setSendMenuPos(null)}
              items={[
                { type: 'label', text: t('schedule_send', { defaultValue: 'Programmer l\'envoi' }) },
                ...schedulePresets().map<MenuItem>(p => ({ type: 'action', label: p.label, onClick: () => sendMut.mutate(p.at) })),
              ]}
            />
          )}
        </div>

        {/* Aa toggle */}
        <button
          onClick={() => setShowFmt(v => !v)}
          className={`w-9 h-9 flex items-center justify-center rounded-full text-sm font-semibold transition-colors flex-shrink-0 ${
            showFmt ? 'bg-primary/10 text-primary' : 'bg-surface-2 text-text-secondary hover:bg-surface-3'
          }`}
          title={t('mail_formatting')}
        >
          Aa
        </button>

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
        <IconBtn title={t('mail_signature', { defaultValue: 'Insérer une signature' })} onClick={insertSignature}><Type size={15} /></IconBtn>
        <IconBtn title={t('more_options')} onClick={e => { const r = (e!.currentTarget as HTMLElement).getBoundingClientRect(); setMoreMenu({ top: r.top, left: r.left }) }}><MoreHorizontal size={15} /></IconBtn>

        <div className="flex-1" />

        <button
          onClick={() => setComposeOpen(false)}
          className="p-2 rounded-full hover:bg-danger/10 hover:text-danger text-text-tertiary transition-colors"
          title={t('discard')}
        >
          <Trash2 size={15} />
        </button>
      </div>

      {moreMenu && <MenuDropdown items={moreItems} pos={moreMenu} onClose={() => setMoreMenu(null)} />}
    </FloatingWindow>
  )
}
