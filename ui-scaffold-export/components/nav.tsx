'use client'

import { usePathname } from 'next/navigation'
import Link from 'next/link'
import { cn } from '@/lib/utils'

const links = [
  { href: '/#api', label: 'API' },
  { href: '/#pricing', label: 'Pricing' },
  { href: '/playground', label: 'Playground' },
  { href: '/dashboard', label: 'Dashboard' },
]

export function Nav() {
  const pathname = usePathname()

  return (
    <nav className="border-b border-border sticky top-0 z-50 bg-background/95 backdrop-blur-sm">
      <div className="mx-auto max-w-6xl px-6 md:px-12 h-14 flex items-center justify-between">
        <Link href="/" className="text-sm font-semibold text-text-primary tracking-tight hover:text-accent transition-colors">
          greed<span className="text-accent">.</span>compute
        </Link>

        <div className="flex items-center gap-6">
          {links.map(link => (
            <Link
              key={link.href}
              href={link.href}
              className={cn(
                'text-xs font-mono transition-colors',
                pathname === link.href || pathname.startsWith(link.href.split('#')[0] + '/') && link.href !== '/'
                  ? 'text-text-primary'
                  : 'text-text-secondary hover:text-text-primary'
              )}
            >
              {link.label}
            </Link>
          ))}
          <Link
            href="/upgrade"
            className="px-3 py-1.5 text-xs font-mono bg-accent text-background hover:bg-accent-dim transition-colors"
          >
            Upgrade
          </Link>
        </div>
      </div>
    </nav>
  )
}
