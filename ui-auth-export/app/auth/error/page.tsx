'use client'

import { Suspense } from 'react'
import { useSearchParams } from 'next/navigation'
import { motion } from 'framer-motion'
import { Nav } from '@/components/nav'

const REASONS: Record<string, { title: string; body: string }> = {
  account_too_new: {
    title: 'Account too new',
    body: 'To prevent abuse, we require your GitHub account to be at least 30 days old. Come back once your account has aged a bit.',
  },
  github_auth_failed: {
    title: 'GitHub auth failed',
    body: 'Something went wrong with the GitHub OAuth flow. Please try again.',
  },
}

function ErrorContent() {
  const params = useSearchParams()
  const reason = params.get('reason') || 'github_auth_failed'
  const info   = REASONS[reason] ?? REASONS['github_auth_failed']

  return (
    <motion.div
      initial={{ opacity: 0, y: 16 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.4 }}
      className="w-full max-w-sm"
    >
      <p className="text-xs font-mono text-error uppercase tracking-widest mb-3">Auth error</p>
      <h1 className="text-xl font-semibold text-text-primary mb-3">{info.title}</h1>
      <p className="text-sm text-text-secondary leading-relaxed mb-8">{info.body}</p>
      <a href="/login" className="text-xs text-accent hover:text-accent-dim transition-colors font-mono">
        ← Try again
      </a>
    </motion.div>
  )
}

export default function AuthErrorPage() {
  return (
    <main className="min-h-screen flex flex-col">
      <Nav />
      <div className="flex-1 flex items-center justify-center px-6">
        <Suspense fallback={<div className="text-text-tertiary text-xs font-mono">Loading...</div>}>
          <ErrorContent />
        </Suspense>
      </div>
    </main>
  )
}
