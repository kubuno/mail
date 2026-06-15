import { useNavigate, useLocation } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import { Settings } from 'lucide-react'

// Surcharge du bouton « Paramètres » de l'en-tête (slot topbar-settings) : route vers
// les réglages Mail quand on est dans /mail, sinon les réglages globaux. <button> (et
// non <Link>) + mêmes styles que les autres icônes d'en-tête (cercle de survol #e8eaed
// bien visible, variante compacte/sombre) pour rester parfaitement homogène.
export default function MailTopbarSettingsButton({ compact = false, dark = false }: { compact?: boolean; dark?: boolean } = {}) {
  const { t } = useTranslation('mail')
  const navigate = useNavigate()
  const { pathname } = useLocation()
  const to = pathname.startsWith('/mail') ? '/mail/settings' : '/settings'
  return (
    <button
      onClick={() => navigate(to)}
      title={t('mail_settings')}
      aria-label={t('mail_settings')}
      className={`${compact ? 'w-9 h-9' : 'w-12 h-12'} rounded-full flex items-center justify-center transition-colors focus:outline-none ${
        dark ? 'text-white/75 hover:bg-white/15 data-[state=open]:bg-white/15' : 'text-text-secondary hover:bg-surface-3 data-[state=open]:bg-surface-3'}`}
    >
      <Settings size={compact ? 18 : 20} />
    </button>
  )
}
