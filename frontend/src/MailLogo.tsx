interface MailLogoProps {
  size?:      number
  className?: string
  title?:     string
}

/** Logo Mail : carré arrondi bleu + enveloppe blanche fermée. */
export function MailLogo({ size = 24, className, title = 'Mail' }: MailLogoProps) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 512 512"
      fill="none"
      role="img"
      aria-label={title}
      className={className}
    >
      <title>{title}</title>
      <rect width="512" height="512" rx="114" fill="#2563EB" />
      <rect x="116" y="160" width="280" height="190" rx="28" fill="#FFFFFF" />
      <path d="M132 178 L256 268 L380 178" fill="none" stroke="#2563EB" strokeWidth="14" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  )
}

export default MailLogo
