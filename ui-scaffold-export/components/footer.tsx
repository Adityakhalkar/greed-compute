import Link from 'next/link'

const LINKS = [
  { label: 'Playground', href: '/playground' },
  { label: 'Dashboard', href: '/dashboard' },
  { label: 'API docs', href: '/#api' },
  { label: 'Pricing', href: '/#pricing' },
  { label: 'Health', href: 'https://compute.deep-ml.com/v1/health' },
  { label: 'llms.txt', href: 'https://compute.deep-ml.com/llms.txt' },
]

export function Footer() {
  return (
    <footer className="border-t border-border px-6 py-12 md:px-12 lg:px-24">
      <div className="mx-auto max-w-6xl flex flex-col md:flex-row items-start md:items-center justify-between gap-8">
        <div>
          <p className="text-sm font-semibold text-text-primary mb-1">
            greed<span className="text-accent">.</span>compute
          </p>
          <p className="text-xs text-text-tertiary">Stateful Python for AI agents.</p>
        </div>

        <div className="flex flex-wrap gap-x-6 gap-y-2">
          {LINKS.map(link => (
            <Link
              key={link.label}
              href={link.href}
              className="text-xs text-text-tertiary hover:text-text-secondary transition-colors font-mono"
              {...(link.href.startsWith('http') ? { target: '_blank', rel: 'noreferrer' } : {})}
            >
              {link.label}
            </Link>
          ))}
        </div>

        <p className="text-xs text-text-tertiary font-mono">
          © {new Date().getFullYear()} deep-ml.com
        </p>
      </div>
    </footer>
  )
}
