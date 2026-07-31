// « Envoyer ce fichier par e-mail » — contributed to Drive's share menu (slot
// files-share-actions). Reads the target file from the drive context, fetches
// its bytes and opens the mail composer with the file pre-attached.
import { useTranslation } from 'react-i18next'
import { Mail } from 'lucide-react'
import { api } from '@kubuno/sdk'
import { useFilesOpenWith } from '@kubuno/drive'
import { mailApi } from './api'
import { useMailStore } from './store'

/** Fetches the drive file and opens the composer with it attached. */
export async function composeWithDriveFile(file: { id: string; name: string; mime_type: string; size_bytes: number }) {
  const st = useMailStore.getState()
  // Accounts are normally loaded by MailApp; ensure they exist so send works
  // when composing from outside /mail.
  if (!st.accounts.length) {
    try { st.setAccounts((await mailApi.listAccounts()).accounts) } catch { /* compose still opens */ }
  }
  const resp = await api.get(`/drive/${file.id}/download`, { responseType: 'blob' })
  const content = await new Promise<string>((res) => {
    const r = new FileReader()
    r.onload = () => res(String(r.result).split(',')[1] ?? '')
    r.readAsDataURL(resp.data as Blob)
  })
  useMailStore.getState().setComposeInitial({
    to: [], cc: [], subject: file.name, bodyHtml: '',
    attachments: [{ filename: file.name, mime: file.mime_type || 'application/octet-stream', content, size: file.size_bytes }],
  })
  useMailStore.getState().setComposeOpen(true)
}

export default function MailSendFileAction() {
  const { t }  = useTranslation('mail')
  const file   = useFilesOpenWith()
  if (!file) return null
  return (
    <button
      onClick={() => { void composeWithDriveFile(file) }}
      className="w-full flex items-center gap-3 px-3 py-2 text-xs text-text-primary
                 hover:bg-surface-1 cursor-pointer outline-none transition-colors"
    >
      <Mail size={14} className="text-text-secondary" />
      {t('mail_send_file_action', { defaultValue: 'Envoyer ce fichier par e-mail' })}
    </button>
  )
}
