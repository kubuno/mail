import { useState } from 'react'
import { createPortal } from 'react-dom'
import { useTranslation } from 'react-i18next'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { X, Plus, Pencil, Trash2, FileText, Loader2 } from 'lucide-react'
import { Button, Input, Textarea, ConfirmDialog } from '@ui'
import { prompt, useConfirm } from '@kubuno/sdk'
import { mailApi, apiErrorMessage, type MailTemplate } from './api'
import { useMailStore } from './store'

// « Modèles » manager — reachable from the New menu (« À partir d'un modèle »).
// One modal does both jobs: pick a template to start a message from, and manage
// the list (create / rename / delete). Mounted globally (MailComposeGlobal) so
// it survives the New dropdown closing.
export default function MailTemplatesModal({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation('mail')
  const qc = useQueryClient()
  const { setComposeInitial, setComposeOpen } = useMailStore()
  const { confirm, confirmState, handleConfirm, handleCancel } = useConfirm()

  const { data: templates = [], isLoading } = useQuery({
    queryKey: ['mail-templates'],
    queryFn:  mailApi.listTemplates,
  })

  // Inline create form (name + subject + body). Body is stored as HTML; a plain
  // textarea is enough here — rich templates are usually captured from the
  // composer's « Enregistrer comme modèle » instead.
  const [creating, setCreating] = useState(false)
  const [name,    setName]    = useState('')
  const [subject, setSubject] = useState('')
  const [body,    setBody]    = useState('')
  const [error,   setError]   = useState('')

  const refresh = () => qc.invalidateQueries({ queryKey: ['mail-templates'] })

  const createMut = useMutation({
    mutationFn: () => mailApi.createTemplate({ name: name.trim(), subject, bodyHtml: body }),
    onSuccess: () => { setCreating(false); setName(''); setSubject(''); setBody(''); setError(''); refresh() },
    onError:   (e) => setError(apiErrorMessage(e, t('mail_template_save_failed', { defaultValue: "Le modèle n'a pas pu être enregistré." }))),
  })

  const use = (tpl: MailTemplate) => {
    setComposeInitial({ to: [], cc: [], subject: tpl.subject, bodyHtml: tpl.bodyHtml })
    setComposeOpen(true)
    onClose()
  }

  const rename = async (tpl: MailTemplate) => {
    const next = await prompt({
      title:        t('mail_template_rename', { defaultValue: 'Renommer le modèle' }),
      defaultValue: tpl.name,
    })
    if (!next?.trim() || next.trim() === tpl.name) return
    await mailApi.updateTemplate(tpl.id, { name: next.trim(), subject: tpl.subject, bodyHtml: tpl.bodyHtml }).catch(() => {})
    refresh()
  }

  const remove = async (tpl: MailTemplate) => {
    const ok = await confirm({
      title:        t('mail_template_delete', { defaultValue: 'Supprimer le modèle' }),
      message:      t('mail_template_delete_confirm', { name: tpl.name, defaultValue: `Supprimer « ${tpl.name} » ?` }),
      confirmLabel: t('common_delete', { defaultValue: 'Supprimer' }),
      variant:      'danger',
    })
    if (!ok) return
    await mailApi.deleteTemplate(tpl.id).catch(() => {})
    refresh()
  }

  return createPortal(
    <div className="fixed inset-0 bg-black/30 z-50 flex items-center justify-center p-4" onClick={onClose}>
      <div
        className="bg-white rounded-xl shadow-xl w-full max-w-[540px] max-h-[80vh] flex flex-col"
        role="dialog" aria-modal="true"
        onClick={e => e.stopPropagation()}
      >
        <div className="flex items-center justify-between px-5 py-4 border-b border-border flex-shrink-0">
          <h2 className="text-[17px] font-medium text-text-primary flex items-center gap-2">
            <FileText size={18} className="text-text-secondary" />
            {t('mail_templates_title', { defaultValue: 'Modèles' })}
          </h2>
          <button onClick={onClose} className="p-1.5 rounded-lg text-text-tertiary hover:text-text-primary hover:bg-surface-2">
            <X size={18} />
          </button>
        </div>

        <div className="flex-1 overflow-y-auto px-5 py-4">
          {isLoading ? (
            <div className="py-10 flex justify-center text-text-tertiary"><Loader2 size={20} className="animate-spin" /></div>
          ) : templates.length === 0 ? (
            <p className="text-sm text-text-tertiary py-6 text-center">
              {t('mail_templates_empty', { defaultValue: 'Aucun modèle. Créez-en un ci-dessous.' })}
            </p>
          ) : (
            <ul className="space-y-1.5">
              {templates.map(tpl => (
                <li key={tpl.id} className="group flex items-center gap-2 rounded-lg border border-border hover:border-primary/40 hover:bg-surface-1 transition-colors">
                  <button onClick={() => use(tpl)} className="flex-1 min-w-0 text-left px-3 py-2.5">
                    <span className="block text-sm text-text-primary truncate">{tpl.name}</span>
                    {tpl.subject && <span className="block text-xs text-text-tertiary truncate">{tpl.subject}</span>}
                  </button>
                  <div className="flex items-center gap-0.5 pr-2 opacity-0 group-hover:opacity-100 transition-opacity">
                    <button onClick={() => rename(tpl)} title={t('mail_template_rename', { defaultValue: 'Renommer' })}
                      className="p-1.5 rounded text-text-tertiary hover:text-text-primary hover:bg-surface-2"><Pencil size={15} /></button>
                    <button onClick={() => remove(tpl)} title={t('common_delete', { defaultValue: 'Supprimer' })}
                      className="p-1.5 rounded text-text-tertiary hover:text-danger hover:bg-danger/10"><Trash2 size={15} /></button>
                  </div>
                </li>
              ))}
            </ul>
          )}

          {/* Inline create form */}
          {creating ? (
            <div className="mt-4 space-y-2.5 border-t border-border pt-4">
              <Input value={name} onChange={e => setName(e.target.value)}
                placeholder={t('mail_template_name_ph', { defaultValue: 'Nom du modèle' })} />
              <Input value={subject} onChange={e => setSubject(e.target.value)}
                placeholder={t('subject', { defaultValue: 'Objet' })} />
              <Textarea value={body} onChange={e => setBody(e.target.value)}
                placeholder={t('body', { defaultValue: 'Corps du message' })}
                rows={4} className="resize-y" />
              {error && <p className="text-xs text-danger">{error}</p>}
              <div className="flex items-center justify-end gap-2">
                <Button variant="ghost" onClick={() => { setCreating(false); setError('') }}>{t('common_cancel', { defaultValue: 'Annuler' })}</Button>
                <Button disabled={!name.trim() || createMut.isPending} loading={createMut.isPending} onClick={() => createMut.mutate()}>
                  {t('common_create', { defaultValue: 'Créer' })}
                </Button>
              </div>
            </div>
          ) : (
            <button onClick={() => setCreating(true)}
              className="mt-4 flex items-center gap-1.5 text-sm text-primary hover:underline">
              <Plus size={16} /> {t('mail_template_new', { defaultValue: 'Nouveau modèle' })}
            </button>
          )}
        </div>
      </div>

      {confirmState && <ConfirmDialog {...confirmState} onConfirm={handleConfirm} onCancel={handleCancel} />}
    </div>,
    document.body,
  )
}
