import { useTranslation } from 'react-i18next'
import { Lock, ShieldOff } from 'lucide-react'
import { EmailMessage } from '../api'
import { formatFullDate } from './helpers'
import { ImportanceMarker } from './rowChrome'

/**
 * The floating "show details" card of a message: same rows as Gmail's table,
 * minus the ones we do not store (mailed-by / signed-by / TLS come from headers
 * the sync never keeps).
 */
export default function MessageDetails({ message, lang, important = false, onClose }: {
  message:    EmailMessage
  lang:       string
  /** Importance is carried by the THREAD, so the card receives it from above. */
  important?: boolean
  onClose:    () => void
}) {
  const { t } = useTranslation('mail')

  // Addresses are clickable mailto links in the theme's blue, as Gmail shows them.
  const LINK = { color: 'var(--color-primary, #1a73e8)' }
  const MailLink = ({ email }: { email: string }) => (
    <a href={`mailto:${email}`} style={LINK} className="hover:underline">{email}</a>
  )
  const addrText = (list: Array<{ name?: string; email: string }>) => (
    <>
      {list.map((a, i) => (
        <span key={a.email + i}>
          {i > 0 && ', '}
          {a.name ? <>{a.name} &lt;<MailLink email={a.email} />&gt;</> : <MailLink email={a.email} />}
        </span>
      ))}
    </>
  )

  const detailRows: [React.ReactNode, React.ReactNode][] = [
    [t('mail_detail_from', { defaultValue: 'De :' }),
     message.from_name
       ? <><span className="font-semibold text-[#202124]">{message.from_name}</span>
           <span className="text-[#5f6368]"> &lt;<MailLink email={message.from_email} />&gt;</span></>
       : <MailLink email={message.from_email} />],
    [t('mail_detail_to', { defaultValue: 'À :' }), addrText(message.to_addresses)],
    ...(message.cc_addresses.length
      ? [[t('mail_detail_cc', { defaultValue: 'Cc :' }), addrText(message.cc_addresses)] as [React.ReactNode, React.ReactNode]]
      : []),
    ...(message.reply_to
      ? [[t('mail_detail_reply_to', { defaultValue: 'répondre à :' }),
          <MailLink key="rt" email={message.reply_to} />] as [React.ReactNode, React.ReactNode]]
      : []),
    [t('mail_detail_date', { defaultValue: 'Date :' }), formatFullDate(message.sent_at ?? message.received_at, lang)],
    [t('mail_detail_subject', { defaultValue: 'Objet :' }), message.subject],
    ...(message.mailed_by
      ? [[t('mail_detail_mailed_by', { defaultValue: 'Envoyé par :' }), message.mailed_by] as [string, string]]
      : []),
    ...(message.signed_by
      ? [[t('mail_detail_signed_by', { defaultValue: 'signé par :' }), message.signed_by] as [string, string]]
      : []),
    ...(message.security
      ? [[t('mail_detail_security', { defaultValue: 'sécurité :' }),
          message.security === 'none'
            ? <span className="inline-flex items-center gap-1.5 text-[#d93025]">
                <ShieldOff size={14} />
                {t('mail_detail_no_tls', { defaultValue: 'Connexion non chiffrée' })}
              </span>
            : <span className="inline-flex items-center gap-1.5">
                <Lock size={14} className="text-[#188038]" />
                {t('mail_detail_tls', { defaultValue: 'Chiffrement standard (TLS)' })}
              </span>] as [React.ReactNode, React.ReactNode]]
      : []),
    // Importance, shown the way Gmail does: the marker itself IS the label.
    ...(important
      ? [[<ImportanceMarker key="imp" active />,
          t('mail_detail_important', { defaultValue: 'Message important, d\u2019après vos habitudes de lecture.' })] as [React.ReactNode, React.ReactNode]]
      : []),
  ]

  return (
    <>
      {/* Click-away catcher — the card floats over the message body. */}
      <div className="fixed inset-0 z-40" onClick={e => { e.stopPropagation(); onClose() }} />
      <div
        onClick={e => e.stopPropagation()}
        className="absolute left-0 top-full z-50 mt-1 max-w-[min(760px,90vw)]
                   rounded-lg border border-[#dadce0] bg-white shadow-[0_2px_6px_2px_rgba(60,64,67,0.15)]
                   px-6 py-4 overflow-x-auto"
      >
        <table className="text-sm text-text-primary border-separate border-spacing-x-3 border-spacing-y-1">
          <tbody>
            {detailRows.map(([label, value], i) => (
              <tr key={i}>
                <td className="align-top text-right text-[#5f6368] whitespace-nowrap">{label}</td>
                <td className="align-top break-words text-[#202124]">{value}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </>
  )
}
