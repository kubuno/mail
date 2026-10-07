import { useState } from 'react'
import { createPortal } from 'react-dom'
import { useTranslation } from 'react-i18next'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { X, Plus, Pencil, Trash2, Users, Loader2 } from 'lucide-react'
import { Button, Input, ConfirmDialog } from '@ui'
import { useConfirm } from '@kubuno/sdk'
import { mailApi, apiErrorMessage, type RecipientGroup } from './api'
import { RecipientField, type AddressSuggestion } from './AddressSuggest'

// « Listes de diffusion » manager — reachable from the New menu (« Nouvelle liste
// de diffusion »). Create / rename / edit members / delete recipient groups.
// Groups then surface in every address field's autocompletion (AddressSuggest).
interface EditorState { id: string | null; name: string; members: AddressSuggestion[] }

export default function MailGroupsModal({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation('mail')
  const qc = useQueryClient()
  const { confirm, confirmState, handleConfirm, handleCancel } = useConfirm()

  const { data: groups = [], isLoading } = useQuery({
    queryKey: ['mail-recipient-groups'],
    queryFn:  mailApi.listRecipientGroups,
  })

  const [editor, setEditor] = useState<EditorState | null>(null)
  const [error,  setError]  = useState('')

  const refresh = () => qc.invalidateQueries({ queryKey: ['mail-recipient-groups'] })

  const saveMut = useMutation({
    mutationFn: () => {
      const dto = {
        name:    editor!.name.trim(),
        members: editor!.members.map(m => ({ email: m.email, name: m.name })),
      }
      return editor!.id
        ? mailApi.updateRecipientGroup(editor!.id, dto)
        : mailApi.createRecipientGroup(dto)
    },
    onSuccess: () => { setEditor(null); setError(''); refresh() },
    onError:   (e) => setError(apiErrorMessage(e, t('mail_group_save_failed', { defaultValue: "La liste n'a pas pu être enregistrée." }))),
  })

  const remove = async (g: RecipientGroup) => {
    const ok = await confirm({
      title:        t('mail_group_delete', { defaultValue: 'Supprimer la liste' }),
      message:      t('mail_group_delete_confirm', { name: g.name, defaultValue: `Supprimer « ${g.name} » ?` }),
      confirmLabel: t('common_delete', { defaultValue: 'Supprimer' }),
      variant:      'danger',
    })
    if (!ok) return
    await mailApi.deleteRecipientGroup(g.id).catch(() => {})
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
            <Users size={18} className="text-text-secondary" />
            {t('mail_groups_title', { defaultValue: 'Listes de diffusion' })}
          </h2>
          <button onClick={onClose} className="p-1.5 rounded-lg text-text-tertiary hover:text-text-primary hover:bg-surface-2">
            <X size={18} />
          </button>
        </div>

        <div className="flex-1 overflow-y-auto px-5 py-4">
          {isLoading ? (
            <div className="py-10 flex justify-center text-text-tertiary"><Loader2 size={20} className="animate-spin" /></div>
          ) : groups.length === 0 && !editor ? (
            <p className="text-sm text-text-tertiary py-6 text-center">
              {t('mail_groups_empty', { defaultValue: 'Aucune liste. Créez-en une ci-dessous.' })}
            </p>
          ) : (
            <ul className="space-y-1.5">
              {groups.map(g => (
                <li key={g.id} className="group flex items-center gap-2 rounded-lg border border-border hover:bg-surface-1 transition-colors">
                  <div className="flex-1 min-w-0 px-3 py-2.5">
                    <span className="block text-sm text-text-primary truncate">{g.name}</span>
                    <span className="block text-xs text-text-tertiary truncate">
                      {g.members.length}&nbsp;{g.members.length > 1
                        ? t('mail_group_members', { defaultValue: 'destinataires' })
                        : t('mail_group_member', { defaultValue: 'destinataire' })}
                    </span>
                  </div>
                  <div className="flex items-center gap-0.5 pr-2 opacity-0 group-hover:opacity-100 transition-opacity">
                    <button onClick={() => { setError(''); setEditor({ id: g.id, name: g.name, members: g.members.map(m => ({ email: m.email, name: m.name })) }) }}
                      title={t('common_edit', { defaultValue: 'Modifier' })}
                      className="p-1.5 rounded text-text-tertiary hover:text-text-primary hover:bg-surface-2"><Pencil size={15} /></button>
                    <button onClick={() => remove(g)} title={t('common_delete', { defaultValue: 'Supprimer' })}
                      className="p-1.5 rounded text-text-tertiary hover:text-danger hover:bg-danger/10"><Trash2 size={15} /></button>
                  </div>
                </li>
              ))}
            </ul>
          )}

          {/* Create / edit form */}
          {editor ? (
            <div className="mt-4 space-y-2.5 border-t border-border pt-4">
              <Input value={editor.name} onChange={e => setEditor({ ...editor, name: e.target.value })}
                placeholder={t('mail_group_name_ph', { defaultValue: 'Nom de la liste' })} />
              <div className="flex items-start rounded-lg border border-border px-3 py-2">
                <RecipientField
                  chips={editor.members}
                  onChange={members => setEditor({ ...editor, members })}
                  placeholder={t('mail_add_recipient', { defaultValue: 'Ajouter des adresses' })}
                />
              </div>
              {error && <p className="text-xs text-danger">{error}</p>}
              <div className="flex items-center justify-end gap-2">
                <Button variant="ghost" onClick={() => { setEditor(null); setError('') }}>{t('common_cancel', { defaultValue: 'Annuler' })}</Button>
                <Button disabled={!editor.name.trim() || saveMut.isPending} loading={saveMut.isPending} onClick={() => saveMut.mutate()}>
                  {editor.id ? t('common_save', { defaultValue: 'Enregistrer' }) : t('common_create', { defaultValue: 'Créer' })}
                </Button>
              </div>
            </div>
          ) : (
            <button onClick={() => { setError(''); setEditor({ id: null, name: '', members: [] }) }}
              className="mt-4 flex items-center gap-1.5 text-sm text-primary hover:underline">
              <Plus size={16} /> {t('mail_group_new', { defaultValue: 'Nouvelle liste de diffusion' })}
            </button>
          )}
        </div>
      </div>

      {confirmState && <ConfirmDialog {...confirmState} onConfirm={handleConfirm} onCancel={handleCancel} />}
    </div>,
    document.body,
  )
}
