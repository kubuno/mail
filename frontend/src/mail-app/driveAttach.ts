import { ModuleServiceRegistry } from '@kubuno/sdk'
import { filesApi } from '@kubuno/drive'

// ── Large-attachment → Drive share link (Gmail parity) ───────────────────────
//
// When a picked file would push the message past the admin-defined max size, we
// do NOT base64-encode it into POST /send. Instead we upload it to the user's
// Drive under `Mail/Attachment/`, mint a PUBLIC "anyone with the link" share
// (read/download, no expiry, no password), and drop that link into the mail.
//
// `@kubuno/drive` (filesApi) is a CORE-provided lib: its specifier is always
// resolvable at runtime via the host import map, independently of whether the
// Drive MODULE (backend at :3101) is installed. Whether Drive is actually
// AVAILABLE is answered by the ModuleServiceRegistry, which only carries the
// services Drive publishes once it has registered — hence the guard below.

/** True when the Drive module is installed and its services are registered. */
export function isDriveAvailable(): boolean {
  return !!ModuleServiceRegistry.get('drive', 'uploadFile')
}

/** Public download URL a recipient can open WITHOUT authentication.
 *  The core proxies `/api/v1/drive/share/:token/download` to Drive's public
 *  handler, which streams the file with an attachment disposition. There is no
 *  standalone web preview page for a share, so the direct download link is the
 *  addressable, auth-free URL — verified by curl. */
export function shareDownloadUrl(token: string): string {
  return `${window.location.origin}/api/v1/drive/share/${token}/download`
}

// Folder ids are resolved once per session and memoised: the first large file
// creates (or reuses) `Mail` then `Mail/Attachment`, the rest reuse the ids.
let attachmentFolderPromise: Promise<string> | null = null

/** Find a direct child folder by name under `parentId`, else create it. Tolerant
 *  of a pre-existing folder (takes the first name match). */
async function resolveChildFolder(name: string, parentId: string | null): Promise<string> {
  const { folders } = await filesApi.listFolders(parentId)
  const existing = folders.find(f => f.name.toLowerCase() === name.toLowerCase())
  if (existing) return existing.id
  const { folder } = await filesApi.createFolder(name, parentId)
  return folder.id
}

/** Resolve (creating as needed) the `Mail/Attachment/` folder id, memoised for
 *  the session. On failure the memo is cleared so a later attempt can retry. */
export function resolveMailAttachmentFolder(): Promise<string> {
  if (!attachmentFolderPromise) {
    attachmentFolderPromise = (async () => {
      const mailId = await resolveChildFolder('Mail', null)
      return resolveChildFolder('Attachment', mailId)
    })().catch(err => {
      attachmentFolderPromise = null
      throw err
    })
  }
  return attachmentFolderPromise
}

export interface DriveShareResult {
  fileId: string
  token:  string
  url:    string
}

/** Upload `file` under `Mail/Attachment/`, then mint a public read/download
 *  share with no expiry and no password (Gmail parity). Reports upload progress
 *  (0..100) through `onProgress`. */
export async function uploadAsShareLink(
  file: File,
  onProgress?: (pct: number) => void,
): Promise<DriveShareResult> {
  const folderId = await resolveMailAttachmentFolder()
  const { file: uploaded } = await filesApi.uploadFile(file, folderId, onProgress)
  const { share } = await filesApi.createShare({ file_id: uploaded.id, can_download: true })
  if (!share.token) throw new Error('share has no public token')
  return { fileId: uploaded.id, token: share.token, url: shareDownloadUrl(share.token) }
}

/** Best-effort removal of a Drive file uploaded for a cancelled/aborted large
 *  attachment, so a scrapped upload leaves nothing behind. Never throws. */
export async function discardUploadedFile(fileId: string): Promise<void> {
  try { await filesApi.deleteFile(fileId) } catch { /* best-effort cleanup */ }
}
