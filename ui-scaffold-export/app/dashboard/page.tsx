'use client'

import { useEffect, useState } from 'react'
import { Nav } from '@/components/nav'
import { api } from '@/lib/api'
import { cn } from '@/lib/utils'

interface UsageData {
  plan: string
  requests: { used: number; limit: number; remaining: number }
  storage: { used_mb: number; limit_mb: number; remaining_mb: number }
  checkpoint_retention_days: number
}

export default function Dashboard() {
  const [apiKey, setApiKey] = useState<string>('')
  const [usage, setUsage] = useState<UsageData | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    const stored = localStorage.getItem('greed_api_key')
    if (stored) setApiKey(stored)
  }, [])

  useEffect(() => {
    if (!apiKey) return
    setLoading(true)
    api.getUsage(apiKey)
      .then(setUsage)
      .catch(() => setError('Invalid API key or network error'))
      .finally(() => setLoading(false))
  }, [apiKey])

  const storagePercent = usage
    ? Math.round((usage.storage.used_mb / usage.storage.limit_mb) * 100)
    : 0
  const requestPercent = usage
    ? Math.round((usage.requests.used / usage.requests.limit) * 100)
    : 0

  return (
    <main>
      <Nav />
      <div className="mx-auto max-w-5xl px-6 py-16 md:px-12">
        {/* Header */}
        <div className="mb-12">
          <h1 className="text-2xl font-semibold text-text-primary mb-1">Dashboard</h1>
          <p className="text-text-secondary text-sm">Monitor usage, manage checkpoints, upgrade plan.</p>
        </div>

        {/* API Key input if not set */}
        {!apiKey && (
          <div className="border border-border bg-surface p-6 mb-8">
            <p className="text-sm text-text-secondary mb-3">Enter your API key to view usage</p>
            <div className="flex gap-2">
              <input
                type="password"
                placeholder="gc-..."
                className="flex-1 bg-background border border-border px-3 py-2 text-sm font-mono text-text-primary placeholder:text-text-tertiary focus:outline-none focus:border-accent"
                onKeyDown={(e) => {
                  if (e.key === 'Enter') {
                    const val = (e.target as HTMLInputElement).value
                    localStorage.setItem('greed_api_key', val)
                    setApiKey(val)
                  }
                }}
              />
              <button
                className="px-4 py-2 bg-accent text-background text-sm font-medium hover:bg-accent-dim transition-colors"
                onClick={() => {
                  const input = document.querySelector('input[type=password]') as HTMLInputElement
                  if (input?.value) {
                    localStorage.setItem('greed_api_key', input.value)
                    setApiKey(input.value)
                  }
                }}
              >
                Connect
              </button>
            </div>
          </div>
        )}

        {error && (
          <div className="border border-error bg-surface px-4 py-3 text-sm text-error mb-8">{error}</div>
        )}

        {loading && (
          <div className="text-text-tertiary text-sm">Loading...</div>
        )}

        {usage && (
          <div className="grid grid-cols-1 md:grid-cols-3 gap-4 mb-12">
            {/* Plan */}
            <div className="border border-border bg-surface p-6">
              <p className="text-xs uppercase tracking-widest text-text-tertiary mb-2">Plan</p>
              <p className="text-2xl font-semibold text-text-primary capitalize">{usage.plan}</p>
              {usage.plan === 'free' && (
                <a
                  href="/upgrade"
                  className="mt-3 inline-block text-xs text-accent hover:text-accent-dim transition-colors"
                >
                  Upgrade to Pro →
                </a>
              )}
            </div>

            {/* Requests */}
            <div className="border border-border bg-surface p-6">
              <p className="text-xs uppercase tracking-widest text-text-tertiary mb-2">Requests today</p>
              <p className="text-2xl font-semibold text-text-primary tabular-nums">
                {usage.requests.used}
                <span className="text-text-tertiary text-base font-normal"> / {usage.requests.limit === 2147483647 ? '∞' : usage.requests.limit}</span>
              </p>
              <div className="mt-3 h-1 bg-border-strong">
                <div
                  className={cn('h-full transition-all', requestPercent > 80 ? 'bg-error' : 'bg-accent')}
                  style={{ width: `${Math.min(requestPercent, 100)}%` }}
                />
              </div>
            </div>

            {/* Storage */}
            <div className="border border-border bg-surface p-6">
              <p className="text-xs uppercase tracking-widest text-text-tertiary mb-2">Checkpoint storage</p>
              <p className="text-2xl font-semibold text-text-primary tabular-nums">
                {usage.storage.used_mb.toFixed(1)}
                <span className="text-text-tertiary text-base font-normal"> / {usage.storage.limit_mb} MB</span>
              </p>
              <div className="mt-3 h-1 bg-border-strong">
                <div
                  className={cn('h-full transition-all', storagePercent > 80 ? 'bg-error' : 'bg-accent')}
                  style={{ width: `${Math.min(storagePercent, 100)}%` }}
                />
              </div>
              <p className="mt-2 text-xs text-text-tertiary">
                Retention: {usage.checkpoint_retention_days} days
              </p>
            </div>
          </div>
        )}

        {/* Quick links */}
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
    </main>
  )
}
