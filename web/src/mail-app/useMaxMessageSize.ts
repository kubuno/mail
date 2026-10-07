import { useQuery } from '@tanstack/react-query'
import { api } from '@kubuno/sdk'

// ── Admin-defined max message size ───────────────────────────────────────────
//
// `mail.server_max_message_mb` is a PUBLIC setting (module.toml → is_public),
// so it is served by GET /api/v1/config to any authenticated module without an
// admin round-trip. It counts the base64-encoded payload (~+1/3 vs the raw
// file). We fall back to 25 Mo when the key is absent (older seed / Drive not
// yet re-seeded).

/** Fallback matching the setting's factory default (Mo). */
export const DEFAULT_MAX_MESSAGE_MB = 25

/** Reads `mail.server_max_message_mb` from the public config. Returns the limit
 *  in Mo and the equivalent base64 byte budget. */
export function useMaxMessageSize(): { maxMb: number; maxBytes: number } {
  const { data } = useQuery({
    queryKey: ['public-config'],
    queryFn: () => api.get<{ config: Record<string, unknown> }>('/config').then(r => r.data.config),
    staleTime: 5 * 60_000,
  })
  const raw = data?.['mail.server_max_message_mb']
  const maxMb = typeof raw === 'number' && raw > 0 ? raw : DEFAULT_MAX_MESSAGE_MB
  return { maxMb, maxBytes: maxMb * 1024 * 1024 }
}
