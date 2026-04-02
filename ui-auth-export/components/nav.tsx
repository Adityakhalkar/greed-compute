'use client'

import { useEffect, useState } from 'react'
import { usePathname } from 'next/navigation'
import Link from 'next/link'
import { cn } from '@/lib/utils'

const NAV_LINKS = [
  { href: '/#api',       label: 'API'        },
  { href: '/#pricing',   label: 'Pricing'    },
  { href: '/playground', label: 'Playground' },
]

export function Nav() {
  const pathname = usePathname()
  const [hasKey, setHasKey] = useState(false)

  useEffect(() => {
    const check = () => setHasKey(!!localStorage.getItem('greed_api_key'))
    check()
    window.addEventListener('storage', check)
    return () => window.removeEventListener('storage', check)
  }, [pathname])

  return (
    <nav className="border-b border-border sticky top-0 z-50 bg-background/95 backdrop-blur-sm">
      <div className="mx-auto max-w-6xl px-6 md:px-12 h-14 flex items-center justify-between">
        <Link
          href="/"
          className="text-sm font-semibold text-text-primary tracking-tight hover:text-accent transition-colors"
        >
          greed<span className="text-accent">.</span>compute
        </Link>

        <div className="flex items-center gap-6">
          {NAV_LINKS.map(link => (
            <Link
              key={link.href}
              href={link.href}
              className={cn(
                'text-xs font-mono transition-colors hidden sm:block',
                pathname === link.href
                  ? 'text-text-primary'
                  : 'text-text-secondary hover:text-text-primary'
              )}
            >
              {link.label}
            </Link>
          ))}

          {hasKey ? (
            <Link
              href="/dashboard"
              className="px-3 py-1.5 text-xs font-mono border border-border text-text-secondary hover:border-border-strong hover:text-text-primary transition-colors"
            >
              Dashboard
            </Link>
          ) : (
            <Link
              href="/login"
              className="px-3 py-1.5 text-xs font-mono bg-accent text-background hover:bg-accent-dim transition-colors"
            >
              Sign in
            </Link>
          )}
        </div>
      </div>
    </nav>
  )
}
