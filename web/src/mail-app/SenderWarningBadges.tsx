import { useTranslation } from 'react-i18next'
import { ShieldAlert, Globe } from 'lucide-react'
import type { SenderSafety } from './senderSafety'

/**
 * Impersonation warnings for the open message, in the same chip language as the
 * OpenPGP trust badges right next to them: a quiet coloured pill, the detail in
 * its tooltip. No dialog and no full-width banner — the reader must be informed
 * without the client crying wolf on every newsletter with an accented domain.
 */
export default function SenderWarningBadges({ safety }: { safety: SenderSafety }) {
  const { t } = useTranslation('mail')
  if (!safety.suspicious) return null

  const chip = 'inline-flex items-center gap-1 h-5 px-2 rounded-full text-[11px] font-medium'

  return (
    <div className="mt-1 flex flex-wrap items-center gap-1.5">
      {safety.spoofedAddress && (
        <span
          className={`${chip} bg-[#fce8e6] text-[#c5221f]`}
          title={t('mail_sender_spoof_hint', {
            defaultValue:
              'Le nom affiché contient l’adresse « {{claimed}} », qui n’est pas celle de l’expéditeur ({{real}}).',
            claimed: safety.spoofedAddress,
            real:    safety.email,
          })}
        >
          <ShieldAlert size={12} />
          {t('mail_sender_spoof', { defaultValue: 'Nom d’expéditeur trompeur' })}
        </span>
      )}
      {safety.idn && (
        <span
          className={`${chip} bg-[#fef7e0] text-[#b06000]`}
          title={safety.idn.mixedScript
            ? t('mail_sender_idn_mixed_hint', {
                defaultValue:
                  'Le domaine « {{unicode}} » mélange plusieurs alphabets et imite peut-être un domaine connu. Forme réelle : {{ascii}}',
                unicode: safety.idn.unicode,
                ascii:   safety.idn.ascii,
              })
            : t('mail_sender_idn_hint', {
                defaultValue: 'Domaine international « {{unicode}} ». Forme réelle : {{ascii}}',
                unicode: safety.idn.unicode,
                ascii:   safety.idn.ascii,
              })}
        >
          <Globe size={12} />
          {t('mail_sender_idn', { defaultValue: 'Domaine internationalisé' })}
        </span>
      )}
    </div>
  )
}
