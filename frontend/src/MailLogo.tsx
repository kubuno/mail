interface MailLogoProps {
  size?:      number
  className?: string
  title?:     string
}

/** Mail logo: raster PNG served by the host at the root (like the favicon and
 *  the Office sub-module logos). Kept as a component so every call site
 *  (waffle menu, icon slots) stays unchanged. */
export function MailLogo({ size = 24, className, title = 'Mail' }: MailLogoProps) {
  return (
    <img
      src="/mail-logo.png"
      width={size}
      height={size}
      alt={title}
      className={className}
      draggable={false}
    />
  )
}

export default MailLogo
