'use client'

import { useEffect } from 'react'
import { useRouter } from 'next/navigation'
import { motion } from 'framer-motion'
import { Nav } from '@/components/nav'

// GitHub mark SVG — inline, no lucide
function GitHubMark({ className }: { className?: string }) {
  return (
    <svg className={className} viewBox="0 0 24 24" fill="currentColor" aria-hidden>
      <path d="M12 0C5.37 0 0 5.37 0 12c0 5.31 3.435 9.795 8.205 11.385.6.105.825-.255.825-.57 0-.285-.015-1.23-.015-2.235-3.015.555-3.795-.735-4.035-1.41-.135-.345-.72-1.41-1.23-1.695-.42-.225-1.02-.78-.015-.795.945-.015 1.62.87 1.845 1.23 1.08 1.815 2.805 1.305 3.495.99.105-.78.42-1.305.765-1.605-2.67-.3-5.46-1.335-5.46-5.925 0-1.305.465-2.385 1.23-3.225-.12-.3-.54-1.53.12-3.18 0 0 1.005-.315 3.3 1.23.96-.27 1.98-.405 3-.405s2.04.135 3 .405c2.295-1.56 3.3-1.23 3.3-1.23.66 1.65.24 2.88.12 3.18.765.84 1.23 1.905 1.23 3.225 0 4.605-2.805 5.625-5.475 5.925.435.375.81 1.095.81 2.22 0 1.605-.015 2.895-.015 3.3 0 .315.225.69.825.57A12.02 12.02 0 0 0 24 12c0-6.63-5.37-12-12-12z" />
    </svg>
  )
}

export default function LoginPage() {
  const router = useRouter()
  const apiBase = process.env.NEXT_PUBLIC_API_URL || 'https://compute.deep-ml.com'
  const authUrl = `${apiBase}/v1/auth/github`

  useEffect(() => {
    if (localStorage.getItem('greed_api_key')) {
      router.replace('/dashboard')
    }
  }, [router])

  return (
    <main className="min-h-screen flex flex-col">
      <Nav />
      <div className="flex-1 flex items-center justify-center px-6">
        <motion.div
          initial={{ opacity: 0, y: 16 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: 0.4 }}
          className="w-full max-w-sm"
        >
          <p className="text-sm font-semibold text-text-primary mb-1">
            greed<span className="text-accent">.</span>compute
          </p>
          <h1 className="text-2xl font-semibold text-text-primary mb-2">Sign in</h1>
          <p className="text-sm text-text-secondary mb-8">
            One click. No password. Your API key is waiting.
          </p>

          <a
            href={authUrl}
            className="flex items-center justify-center gap-3 w-full py-3 px-5 bg-surface border border-border hover:border-border-strong text-text-primary text-sm font-medium transition-colors group"
          >
            <GitHubMark className="w-4 h-4 text-text-secondary group-hover:text-text-primary transition-colors" />
            Continue with GitHub
          </a>

          <p className="mt-6 text-xs text-text-tertiary text-center leading-relaxed">
            Only your GitHub username and email are read.
            We never access your repositories.
            Accounts must be at least 30 days old.
          </p>
        </motion.div>
      </div>
    </main>
  )
}
