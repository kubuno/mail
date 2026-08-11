import { useCallback, useEffect, useRef, useState } from 'react'
import { isDriveAvailable, uploadAsShareLink, discardUploadedFile } from './driveAttach'

// ── Compose attachments (shared by ComposeWindow + InlineCompose) ─────────────
//
// Two kinds of attachment coexist:
//
//  • `kind: 'file'` — the classic path. The file is read + base64-encoded
//    LOCALLY (FileReader → progress events) and travels inside the POST /send
//    JSON payload. The bytes only leave the browser when the message is sent.
//
//  • `kind: 'link'` — Gmail parity for a file that would push the message past
//    the admin's max size. Instead of base64, it is uploaded to the user's
//    Drive under `Mail/Attachment/` and a PUBLIC share link is minted; that URL
//    is injected into the mail body at send time (see driveLinks()). Such an
//    attachment NEVER enters the /send `attachments` array.
//
// The size decision is taken in `addFiles`: the estimated base64 size
// (`file.size * 4/3`) is added to the running total of ready base64 files; if
// that would exceed the budget, the file is routed to Drive.

export type AttachmentKind = 'file' | 'link'
export type AttachmentErrorKind = 'generic' | 'drive-missing'

export interface ComposeAttachment {
  id:       string
  filename: string
  mime:     string
  size:     number
  kind:     AttachmentKind
  /** Progress 0..100. For files: local base64 encoding. For links: Drive upload. */
  progress: number
  /** 'reading' = encoding (file) or uploading (link); then 'ready' | 'error'. */
  status:   'reading' | 'ready' | 'error'
  /** Base64 (no `data:` prefix). Populated once a FILE is ready. Empty for links. */
  content:  string
  /** File the user picked, so we can build an object URL for preview/download. */
  file?:    File
  /** Link only: public Drive download URL, ready once the upload+share succeed. */
  driveUrl?:    string
  /** Link only: Drive file id, kept for best-effort cleanup on cancel. */
  driveFileId?: string
  /** Why a 'error' attachment failed, so the chip can phrase it. */
  errorKind?:   AttachmentErrorKind
}

/** Wire format expected by POST /send. */
export type AttachmentPayload = { filename: string; mime: string; content: string }

/** A ready Drive-link attachment, for injecting the URL into the mail body. */
export type DriveLink = { filename: string; size: number; url: string }

/** Prefilled attachment (already base64-encoded, e.g. « send by email » from Drive). */
export type PrefilledAttachment = { filename: string; mime: string; content: string; size: number }

export interface ComposeAttachmentsOptions {
  /** Base64 byte budget (admin max message size). A file whose estimated encoded
   *  size would exceed the remaining budget is routed to Drive as a link. */
  maxBytes?: number
}

let seq = 0
const nextId = () => `att-${Date.now()}-${seq++}`

/** Base64-encoded byte length of a raw byte count (4 chars per 3 bytes). */
const base64Len = (bytes: number) => Math.ceil(bytes / 3) * 4

/**
 * Owns the enriched attachment list for a composer. Small files are base64
 * encoded locally (FileReader); a file over the admin size budget is uploaded
 * to Drive and carried as a public link. Reads/uploads in flight can be
 * cancelled. Object URLs (local preview/download) are created lazily and
 * revoked on removal and on unmount.
 */
export function useComposeAttachments(
  initial?: PrefilledAttachment[],
  opts?: ComposeAttachmentsOptions,
) {
  const [attachments, setAttachments] = useState<ComposeAttachment[]>(() =>
    (initial ?? []).map(a => ({
      id: nextId(), filename: a.filename, mime: a.mime, size: a.size, kind: 'file' as const,
      progress: 100, status: 'ready' as const, content: a.content,
    })))

  // Mirror of the list, so addFiles can measure the running base64 total without
  // depending on `attachments` (which would re-create the callback each keystroke).
  const attachmentsRef = useRef(attachments)
  useEffect(() => { attachmentsRef.current = attachments }, [attachments])

  // Live max-size budget, read through a ref so addFiles stays stable.
  const maxBytesRef = useRef<number | undefined>(opts?.maxBytes)
  useEffect(() => { maxBytesRef.current = opts?.maxBytes }, [opts?.maxBytes])

  // Active FileReaders by id, so an in-progress read can be aborted.
  const readersRef = useRef<Map<string, FileReader>>(new Map())
  // Ids removed while their Drive upload was still in flight — the resolving
  // upload checks this to skip the state update and clean the orphaned file.
  const cancelledRef = useRef<Set<string>>(new Set())
  // Object URLs created for preview/download, revoked on removal / unmount.
  const urlsRef = useRef<Map<string, string>>(new Map())

  const revokeUrl = useCallback((id: string) => {
    const url = urlsRef.current.get(id)
    if (url) { URL.revokeObjectURL(url); urlsRef.current.delete(id) }
  }, [])

  // Reads a small file into base64, feeding the chip's progress bar.
  const readAsFile = useCallback((id: string, file: File) => {
    const reader = new FileReader()
    readersRef.current.set(id, reader)
    reader.onprogress = e => {
      if (!e.lengthComputable) return
      const pct = Math.min(99, Math.round((e.loaded / e.total) * 100))
      setAttachments(prev => prev.map(a =>
        a.id === id && a.status === 'reading' ? { ...a, progress: pct } : a))
    }
    reader.onload = () => {
      readersRef.current.delete(id)
      const content = String(reader.result).split(',')[1] ?? ''
      setAttachments(prev => prev.map(a =>
        a.id === id ? { ...a, content, progress: 100, status: 'ready' } : a))
    }
    reader.onerror = () => {
      readersRef.current.delete(id)
      setAttachments(prev => prev.map(a =>
        a.id === id ? { ...a, status: 'error', errorKind: 'generic' } : a))
    }
    reader.readAsDataURL(file)
  }, [])

  // Uploads an oversized file to Drive and mints a public link, feeding progress.
  const uploadAsLink = useCallback((id: string, file: File) => {
    if (!isDriveAvailable()) {
      // Can't send it base64 (over the SMTP limit) and Drive is absent: fail clearly.
      setAttachments(prev => prev.map(a =>
        a.id === id ? { ...a, status: 'error', errorKind: 'drive-missing' } : a))
      return
    }
    uploadAsShareLink(file, pct => {
      if (cancelledRef.current.has(id)) return
      const capped = Math.min(99, pct)
      setAttachments(prev => prev.map(a =>
        a.id === id && a.status === 'reading' ? { ...a, progress: capped } : a))
    })
      .then(res => {
        // Cancelled mid-flight: drop the orphaned Drive file, skip the update.
        if (cancelledRef.current.has(id)) { void discardUploadedFile(res.fileId); return }
        setAttachments(prev => prev.map(a =>
          a.id === id ? { ...a, status: 'ready', progress: 100, driveUrl: res.url, driveFileId: res.fileId } : a))
      })
      .catch(() => {
        if (cancelledRef.current.has(id)) return
        setAttachments(prev => prev.map(a =>
          a.id === id ? { ...a, status: 'error', errorKind: 'generic' } : a))
      })
  }, [])

  const addFiles = useCallback((files: FileList | File[] | null) => {
    const list = files ? [...files] : []
    if (!list.length) return

    const budget = maxBytesRef.current
    // Running base64 total of everything already staged as an inline file.
    let used = attachmentsRef.current
      .filter(a => a.kind === 'file' && a.status !== 'error')
      .reduce((n, a) => n + base64Len(a.size), 0)

    for (const file of list) {
      const id = nextId()
      const encoded = base64Len(file.size)
      // Route to Drive when a real budget is known and this file would overflow it.
      const asLink = !!budget && used + encoded > budget
      if (!asLink) used += encoded

      setAttachments(prev => [...prev, {
        id, filename: file.name, mime: file.type || 'application/octet-stream',
        size: file.size, kind: asLink ? 'link' : 'file',
        progress: 0, status: 'reading', content: '', file,
      }])

      if (asLink) uploadAsLink(id, file)
      else readAsFile(id, file)
    }
  }, [readAsFile, uploadAsLink])

  // Removes an attachment: aborts a read still in flight (= "cancel"), marks a
  // Drive upload still in flight as cancelled (best-effort abort + cleanup),
  // revokes its object URL, drops it from the list. A cancelled item never
  // reaches 'ready', so it can never enter the /send payload or the body links.
  const remove = useCallback((id: string) => {
    const reader = readersRef.current.get(id)
    if (reader) { reader.abort(); readersRef.current.delete(id) }
    const att = attachmentsRef.current.find(a => a.id === id)
    if (att?.kind === 'link') {
      if (att.status === 'reading') cancelledRef.current.add(id)
      // Already uploaded then removed: reclaim the Drive file.
      if (att.driveFileId) void discardUploadedFile(att.driveFileId)
    }
    revokeUrl(id)
    setAttachments(prev => prev.filter(a => a.id !== id))
  }, [revokeUrl])

  // Lazily builds (and caches) an object URL for local preview/download.
  const objectUrl = useCallback((att: ComposeAttachment): string => {
    const existing = urlsRef.current.get(att.id)
    if (existing) return existing
    let blob: Blob
    if (att.file) {
      blob = att.file
    } else {
      // Prefilled attachment: no File in hand, decode its base64 into a Blob.
      const bin = atob(att.content)
      const bytes = new Uint8Array(bin.length)
      for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i)
      blob = new Blob([bytes], { type: att.mime })
    }
    const url = URL.createObjectURL(blob)
    urlsRef.current.set(att.id, url)
    return url
  }, [])

  // Ready inline (base64) attachments in the wire format POST /send expects.
  // Drive-link attachments are excluded on purpose — they travel as body links.
  const payload = useCallback((): AttachmentPayload[] =>
    attachments
      .filter(a => a.kind === 'file' && a.status === 'ready')
      .map(a => ({ filename: a.filename, mime: a.mime, content: a.content })),
    [attachments])

  // Ready Drive-link attachments, for injecting their public URLs into the body.
  const driveLinks = useCallback((): DriveLink[] =>
    attachments
      .filter(a => a.kind === 'link' && a.status === 'ready' && a.driveUrl)
      .map(a => ({ filename: a.filename, size: a.size, url: a.driveUrl! })),
    [attachments])

  // Large-file uploads currently in flight — drives the « Ajout des fichiers… » modal.
  const pendingUploads = attachments.filter(a => a.kind === 'link' && a.status === 'reading')

  // Teardown: abort pending reads, revoke every object URL.
  useEffect(() => {
    const readers = readersRef.current
    const urls = urlsRef.current
    return () => {
      readers.forEach(r => r.abort())
      readers.clear()
      urls.forEach(u => URL.revokeObjectURL(u))
      urls.clear()
    }
  }, [])

  return { attachments, addFiles, remove, objectUrl, payload, driveLinks, pendingUploads }
}
