'use client'

import { Suspense } from 'react'
import { useEffect, useState } from 'react'
import { useSearchParams } from 'next/navigation'
import { motion, AnimatePresence } from 'framer-motion'
import { toast } from 'sonner'
import { Nav } from '@/components/nav'
import { api } from '@/lib/api'
import { cn } from '@/lib/utils'

interface UsageData {
  plan: string
  requests: { used: number; limit: number; remaining: number }
  storage: { used_mb: number; limit_mb: number; remaining_mb: number }
  checkpoint_retention_days: number
}

function CopyIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
      <rect x="9" y="9" width="13" height="13" rx="1" />
      <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
    </svg>
  )
}

function DashboardContent() {
  const searchParams = useSearchParams()
  const [apiKey, setApiKey]       = useState<string>('')
  const [login, setLogin]         = useState<string>('')
  const [newKey, setNewKey]       = useState<string | null>(null)
  const [keyCopied, setKeyCopied] = useState(false)
  const [usage, setUsage]         = useState<UsageData | null>(null)
  const [loading, setLoading]     = useState(false)
  const [error, setError]         = useState<string | null>(null)

  useEffect(() => {
    const urlKey   = searchParams.get('key')
    const urlLogin = searchParams.get('login')

    if (urlKey) {
      const isNew = !localStorage.getItem('greed_api_key')
      localStorage.setItem('greed_api_key', urlKey)
      setApiKey(urlKey)
      if (isNew) setNewKey(urlKey)
      window.history.replaceState({}, '', '/dashboard')
    } else {
      const stored = localStorage.getItem('greed_api_key')
      if (stored) setApiKey(stored)
    }

    if (urlLogin) {
      localStorage.setItem('greed_login', urlLogin)
      setLogin(urlLogin)
    } else {
      setLogin(localStorage.getItem('greed_login') || '')
    }
  }, [searchParams])

  useEffect(() => {
    if (!apiKey) return
    setLoading(true)
    api.getUsage(apiKey)
      .then(setUsage)
      .catch(() => setError('Could not load usage — check your key'))
      .finally(() => setLoading(false))
  }, [apiKey])

  const copyKey = (key: string) => {
    navigator.clipboard.writeText(key)
    setKeyCopied(true)
    toast.success('API key copied')
    setTimeout(() => setKeyCopied(false), 2000)
  }

  const signOut = () => {
    localStorage.removeItem('greed_api_key')
    localStorage.removeItem('greed_login')
    window.location.href = '/login'
  }

  const storagePercent = usage ? Math.round((usage.storage.used_mb / usage.storage.limit_mb) * 100) : 0
  const requestPercent = usage ? Math.round((usage.requests.used / usage.requests.limit) * 100) : 0

  if (!apiKey) {
    return (
      <div className="mx-auto max-w-5xl px-6 py-24 text-center">
        <p className="text-text-secondary text-sm mb-4">You're not signed in.</p>
        <a href="/login" className="text-accent text-sm hover:text-accent-dim transition-colors">
          Sign in with GitHub →
        </a>
      </div>
    )
  }

  return (
    <div className="mx-auto max-w-5xl px-6 py-16 md:px-12">
      <div className="flex items-start justify-between mb-10">
        <div>
          <h1 className="text-2xl font-semibold text-text-primary mb-1">Dashboard</h1>
          {login && <p className="text-xs text-text-tertiary font-mono">@{login}</p>}
        </div>
        <button onClick={signOut} className="text-xs text-text-tertiary hover:text-text-secondary transition-colors font-mono">
          Sign out
        </button>
      </div>

      <AnimatePresence>
        {newKey && (
          <motion.div
            initial={{ opacity: 0, y: -8 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -8 }}
            transition={{ duration: 0.3 }}
            className="border border-accent bg-surface p-5 mb-8"
          >
            <p className="text-xs text-accent font-mono tracking-widest uppercase mb-3">Your API key</p>
            <p className="text-xs text-text-secondary mb-4 leading-relaxed">
              Copy this now. For security we won't show it again — sign in again anytime to retrieve it.
            </p>
            <div className="flex items-center gap-3">
              <code className="flex-1 bg-background border border-border px-3 py-2 text-xs font-mono text-text-primary tracking-wider overflow-x-auto">
                {newKey}
              </code>
              <button
                onClick={() => copyKey(newKey)}
                className="shrink-0 flex items-center gap-2 px-3 py-2 bg-accent text-background text-xs font-medium hover:bg-accent-dim transition-colors"
              >
                <CopyIcon />
                {keyCopied ? 'Copied!' : 'Copy'}
              </button>
            </div>
            <button onClick={() => setNewKey(null)} className="mt-3 text-xs text-text-tertiary hover:text-text-secondary transition-colors">
              I've saved it, dismiss
            </button>
          </motion.div>
        )}
      </AnimatePresence>

      {error && <div className="border border-error bg-surface px-4 py-3 text-sm text-error mb-8">{error}</div>}
      {loading && <p className="text-text-tertiary text-sm">Loading...</p>}

      {usage && (
        <div className="grid grid-cols-1 md:grid-cols-3 gap-4 mb-12">
          <div className="border border-border bg-surface p-6">
            <p className="text-xs uppercase tracking-widest text-text-tertiary mb-2">Plan</p>
            <p className="text-2xl font-semibold text-text-primary capitalize">{usage.plan}</p>
            {usage.plan === 'free' && (
              <a href="/upgrade" className="mt-3 inline-block text-xs text-accent hover:text-accent-dim transition-colors">Upgrade to Pro →</a>
            )}
          </div>
          <div className="border border-border bg-surface p-6">
            <p className="text-xs uppercase tracking-widest text-text-tertiary mb-2">Requests today</p>
            <p className="text-2xl font-semibold text-text-primary tabular-nums">
              {usage.requests.used}
              <span className="text-text-tertiary text-base font-normal"> / {usage.requests.limit === 2147483647 ? '∞' : usage.requests.limit}</span>
            </p>
            <div className="mt-3 h-1 bg-border-strong">
              <div className={cn('h-full transition-all', requestPercent > 80 ? 'bg-error' : 'bg-accent')} style={{ width: `${Math.min(requestPercent, 100)}%` }} />
            </div>
          </div>
          <div className="border border-border bg-surface p-6">
            <p className="text-xs uppercase tracking-widest text-text-tertiary mb-2">Checkpoint storage</p>
            <p className="text-2xl font-semibold text-text-primary tabular-nums">
              {usage.storage.used_mb.toFixed(1)}
              <span className="text-text-tertiary text-base font-normal"> / {usage.storage.limit_mb} MB</span>
            </p>
            <div className="mt-3 h-1 bg-border-strong">
              <div className={cn('h-full transition-all', storagePercent > 80 ? 'bg-error' : 'bg-accent')} style={{ width: `${Math.min(storagePercent, 100)}%` }} />
            </div>
            <p className="mt-2 text-xs text-text-tertiary">Retention: {usage.checkpoint_retention_days} days</p>
          </div>
        </div>
      )}

      <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
        <a href="/playground" className="border border-border bg-surface p-6 hover:border-border-strong transition-colors group">
          <p className="text-sm font-medium text-text-primary mb-1 group-hover:text-accent transition-colors">Playground →</p>
          <p className="text-xs text-text-secondary">Run Python in your browser against a live session</p>
        </a>
        <a href="/upgrade" className="border border-border bg-surface p-6 hover:border-border-strong transition-colors group">
          <p className="text-sm font-medium text-text-primary mb-1 group-hover:text-accent transition-colors">Upgrade plan →</p>
          <p className="text-xs text-text-secondary">More storage, higher rate limits, longer retention</p>
        </a>
      </div>
    </div>
  )
}

export default function Dashboard() {
  return (
    <main>
      <Nav />
      <Suspense fallback={<div className="mx-auto max-w-5xl px-6 py-16 text-text-tertiary text-sm">Loading...</div>}>
        <DashboardContent />
      </Suspense>
    </main>
  )
}
