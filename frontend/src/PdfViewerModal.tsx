import { useCallback, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import * as pdfjsLib from 'pdfjs-dist'
import type { PDFDocumentProxy, PDFDocumentLoadingTask } from 'pdfjs-dist'
import {
  X, Printer, Download, FileText,
  ChevronLeft, ChevronRight, ZoomIn, ZoomOut, Loader2,
} from 'lucide-react'
import { useAuthStore } from '@kubuno/sdk'

pdfjsLib.GlobalWorkerOptions.workerSrc = new URL(
  'pdfjs-dist/build/pdf.worker.min.mjs',
  import.meta.url,
).href

interface Props {
  url:      string
  filename: string
  onClose:  () => void
}

export default function PdfViewerModal({ url, filename, onClose }: Props) {
  const { t } = useTranslation()
  const [pdfDoc,    setPdfDoc]    = useState<PDFDocumentProxy | null>(null)
  const [pageNum,   setPageNum]   = useState(1)
  const [numPages,  setNumPages]  = useState(0)
  const [scale,     setScale]     = useState(1.2)
  const [blobUrl,   setBlobUrl]   = useState<string | null>(null)
  const [loading,   setLoading]   = useState(true)
  const [rendering, setRendering] = useState(false)
  const [error,     setError]     = useState<string | null>(null)
  const canvasRef = useRef<HTMLCanvasElement>(null)
  // pdfjs v6 : PDFDocumentProxy n'expose plus `.destroy()` ; le teardown se fait
  // via la tâche de chargement (PDFDocumentLoadingTask.destroy()).
  const loadingTaskRef = useRef<PDFDocumentLoadingTask | null>(null)
  const accessToken = useAuthStore(s => s.accessToken)

  // Fetch + load PDF
  useEffect(() => {
    let cancelled = false
    let localBlob: string | null = null
    setLoading(true)
    setError(null)
    loadingTaskRef.current?.destroy()
    loadingTaskRef.current = null
    setPdfDoc(null)
    setPageNum(1)

    const load = async () => {
      try {
        const resp = await fetch(url, {
          headers: accessToken ? { Authorization: `Bearer ${accessToken}` } : {},
        })
        if (!resp.ok) throw new Error(`HTTP ${resp.status}`)
        const buf = await resp.arrayBuffer()
        if (cancelled) return

        localBlob = URL.createObjectURL(new Blob([buf], { type: 'application/pdf' }))
        setBlobUrl(localBlob)

        const task = pdfjsLib.getDocument({ data: buf })
        loadingTaskRef.current = task
        const doc = await task.promise
        if (cancelled) { task.destroy(); return }

        // Compute initial scale to fit viewport width
        const page0 = await doc.getPage(1)
        if (cancelled) { task.destroy(); return }
        const vp0 = page0.getViewport({ scale: 1 })
        const fitW = (window.innerWidth  - 64)  / vp0.width
        const fitH = (window.innerHeight - 128) / vp0.height
        setScale(Math.min(fitW, fitH, 1.8))

        setPdfDoc(doc)
        setNumPages(doc.numPages)
        setLoading(false)
      } catch (e) {
        if (!cancelled) {
          setError(e instanceof Error ? e.message : t('pdf.error_load'))
          setLoading(false)
        }
      }
    }

    load()
    return () => {
      cancelled = true
      if (localBlob) URL.revokeObjectURL(localBlob)
    }
  }, [url, accessToken])

  // Render current page
  useEffect(() => {
    if (!pdfDoc || !canvasRef.current) return
    let task: ReturnType<ReturnType<PDFDocumentProxy['getPage']>['then']> | null = null
    let cancelled = false
    setRendering(true)

    pdfDoc.getPage(pageNum).then(page => {
      if (cancelled) return
      const vp = page.getViewport({ scale })
      const canvas = canvasRef.current!
      const ctx = canvas.getContext('2d')!
      canvas.width  = vp.width
      canvas.height = vp.height
      const rt = page.render({ canvas, canvasContext: ctx, viewport: vp })
      task = rt.promise.then(() => { if (!cancelled) setRendering(false) }) as never
    }).catch(() => { if (!cancelled) setRendering(false) })

    return () => {
      cancelled = true
      // @ts-expect-error cancel exists on render task promise chain
      if (task?.cancel) (task as unknown as { cancel(): void }).cancel()
    }
  }, [pdfDoc, pageNum, scale])

  // Keyboard navigation
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape')                        onClose()
      if (e.key === 'ArrowLeft' || e.key === 'ArrowUp')
        setPageNum(n => Math.max(1, n - 1))
      if (e.key === 'ArrowRight' || e.key === 'ArrowDown')
        setPageNum(n => Math.min(numPages, n + 1))
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose, numPages])

  const handlePrint = useCallback(() => {
    if (!blobUrl) return
    const iframe = document.createElement('iframe')
    Object.assign(iframe.style, { position: 'fixed', left: '-9999px', top: '-9999px', width: '1px', height: '1px' })
    iframe.src = blobUrl
    document.body.appendChild(iframe)
    iframe.onload = () => {
      iframe.contentWindow?.print()
      setTimeout(() => document.body.removeChild(iframe), 2000)
    }
  }, [blobUrl])

  const zoomIn  = () => setScale(s => Math.min(+(s + 0.2).toFixed(1), 4.0))
  const zoomOut = () => setScale(s => Math.max(+(s - 0.2).toFixed(1), 0.3))

  return (
    <div className="fixed inset-0 z-50 bg-black/90 flex flex-col" onClick={onClose}>

      {/* Top bar */}
      <div
        className="flex items-center justify-between px-4 py-3 bg-black/70 shrink-0"
        onClick={e => e.stopPropagation()}
      >
        <div className="flex items-center gap-2 min-w-0">
          <FileText size={16} className="text-white/60 flex-shrink-0" />
          <p className="text-white text-sm font-medium truncate max-w-[50vw]">{filename}</p>
        </div>
        <div className="flex items-center gap-2">
          <button
            onClick={handlePrint}
            disabled={!blobUrl}
            className="p-2 hover:bg-white/10 rounded-full text-white disabled:opacity-40 transition-colors"
            title={t('pdf.print')}
          >
            <Printer size={16} />
          </button>
          <a
            href={blobUrl ?? '#'}
            download={filename}
            className="flex items-center gap-1.5 px-3 py-1.5 text-sm bg-white/10 hover:bg-white/20 text-white rounded-lg transition-colors"
            onClick={e => { if (!blobUrl) e.preventDefault(); else e.stopPropagation() }}
          >
            <Download size={14} />
            {t('pdf.download')}
          </a>
          <button
            onClick={onClose}
            className="p-2 hover:bg-white/10 rounded-full text-white transition-colors"
          >
            <X size={18} />
          </button>
        </div>
      </div>

      {/* PDF canvas area */}
      <div
        className="flex-1 overflow-auto flex items-start justify-center py-6"
        onClick={e => e.stopPropagation()}
      >
        {loading && (
          <div className="flex items-center gap-2 text-white/70 mt-20">
            <Loader2 size={24} className="animate-spin" />
            <span className="text-sm">{t('common.loading')}</span>
          </div>
        )}
        {error && (
          <div className="flex flex-col items-center gap-2 text-white/60 mt-20">
            <FileText size={40} className="opacity-40" />
            <p className="text-sm">{error}</p>
          </div>
        )}
        {!loading && !error && (
          <div className="relative inline-block">
            {rendering && (
              <div className="absolute inset-0 flex items-center justify-center bg-black/30 z-10 rounded">
                <Loader2 size={20} className="animate-spin text-white/70" />
              </div>
            )}
            <canvas
              ref={canvasRef}
              className="block rounded shadow-2xl"
            />
          </div>
        )}
      </div>

      {/* Bottom navigation bar */}
      {!loading && !error && numPages > 0 && (
        <div
          className="flex items-center justify-center gap-3 px-4 py-2.5 bg-black/70 shrink-0"
          onClick={e => e.stopPropagation()}
        >
          <button
            onClick={() => setPageNum(n => Math.max(1, n - 1))}
            disabled={pageNum <= 1}
            className="p-1.5 hover:bg-white/10 rounded-full text-white disabled:opacity-30 transition-colors"
          >
            <ChevronLeft size={18} />
          </button>
          <span className="text-white/80 text-sm min-w-[80px] text-center">
            {t('pdf.page')} {pageNum} / {numPages}
          </span>
          <button
            onClick={() => setPageNum(n => Math.min(numPages, n + 1))}
            disabled={pageNum >= numPages}
            className="p-1.5 hover:bg-white/10 rounded-full text-white disabled:opacity-30 transition-colors"
          >
            <ChevronRight size={18} />
          </button>

          <div className="w-px h-4 bg-white/20 mx-1" />

          <button onClick={zoomOut} className="p-1.5 hover:bg-white/10 rounded-full text-white transition-colors">
            <ZoomOut size={16} />
          </button>
          <span className="text-white/80 text-sm w-12 text-center">
            {Math.round(scale * 100)}%
          </span>
          <button onClick={zoomIn} className="p-1.5 hover:bg-white/10 rounded-full text-white transition-colors">
            <ZoomIn size={16} />
          </button>
        </div>
      )}
    </div>
  )
}
