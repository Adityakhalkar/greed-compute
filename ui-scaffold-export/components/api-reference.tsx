'use client'

import { useState } from 'react'
import { motion, AnimatePresence } from 'framer-motion'
import { toast } from 'sonner'

const TABS = [
  {
    label: 'Create session',
    method: 'POST',
    path: '/v1/sessions',
    request: `curl -X POST https://compute.deep-ml.com/v1/sessions \\
  -H "X-API-Key: gc-your-key" \\
  -H "Content-Type: application/json" \\
  -d '{"template": "data-science"}'`,
    response: `{
  "session_id": "a3f9c2b1-...",
  "template": "data-science",
  "created_at": 1718000000
}`,
  },
  {
    label: 'Execute code',
    method: 'POST',
    path: '/v1/sessions/{id}/execute',
    request: `curl -X POST https://compute.deep-ml.com/v1/sessions/SESSION_ID/execute \\
  -H "X-API-Key: gc-your-key" \\
  -H "Content-Type: application/json" \\
  -d '{"code": "import numpy as np; np.array([1,2,3]).mean()"}'`,
    response: `{
  "stdout": "",
  "result": 2.0,
  "error": null
}`,
  },
  {
    label: 'Checkpoint',
    method: 'POST',
    path: '/v1/sessions/{id}/checkpoint',
    request: `curl -X POST https://compute.deep-ml.com/v1/sessions/SESSION_ID/checkpoint \\
  -H "X-API-Key: gc-your-key" \\
  -H "Content-Type: application/json" \\
  -d '{"name": "model-loaded"}'`,
    response: `{
  "checkpoint_id": "ckpt_7d2e1f...",
  "name": "model-loaded",
  "size_mb": 142.3
}`,
  },
  {
    label: 'Fork workers',
    method: 'POST',
    path: '/v1/checkpoints/fork',
    request: `curl -X POST https://compute.deep-ml.com/v1/checkpoints/fork \\
  -H "X-API-Key: gc-your-key" \\
  -H "Content-Type: application/json" \\
  -d '{"checkpoint_id": "ckpt_7d2e1f...", "count": 50}'`,
    response: `{
  "session_ids": [
    "sess_00a1...",
    "sess_01b2...",
    "... 48 more"
  ],
  "fork_latency_ms": 78
}`,
  },
]

const METHOD_COLORS: Record<string, string> = {
  GET: 'text-accent',
  POST: 'text-[#7dd3fc]',
  DELETE: 'text-error',
}

export function ApiReference() {
  const [active, setActive] = useState(0)
  const tab = TABS[active]

  const copy = (text: string) => {
    navigator.clipboard.writeText(text)
    toast.success('Copied to clipboard')
  }

  return (
    <section id="api" className="border-t border-border px-6 py-24 md:px-12 lg:px-24 bg-surface">
      <div className="mx-auto max-w-6xl">
        <div className="mb-12">
          <p className="text-xs tracking-widest uppercase text-text-tertiary mb-3 font-mono">API Reference</p>
          <h2 className="text-3xl md:text-4xl font-semibold text-text-primary">REST. Simple.</h2>
        </div>

        <div className="border border-border">
          {/* Tab bar */}
          <div className="flex border-b border-border overflow-x-auto">
            {TABS.map((t, i) => (
              <button
                key={t.label}
                onClick={() => setActive(i)}
                className={`px-4 py-3 text-xs font-mono whitespace-nowrap transition-colors border-r border-border last:border-r-0 ${
                  active === i
                    ? 'bg-background text-text-primary'
                    : 'text-text-tertiary hover:text-text-secondary'
                }`}
              >
                <span className={`${METHOD_COLORS[t.method]} mr-2`}>{t.method}</span>
                {t.label}
              </button>
            ))}
          </div>

          <AnimatePresence mode="wait">
            <motion.div
              key={active}
              initial={{ opacity: 0, y: 4 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -4 }}
              transition={{ duration: 0.15 }}
              className="grid grid-cols-1 lg:grid-cols-2"
            >
              {/* Request */}
              <div className="border-r border-border">
                <div className="flex items-center justify-between px-4 py-2 border-b border-border">
                  <span className="text-xs text-text-tertiary font-mono">Request</span>
                  <button
                    onClick={() => copy(tab.request)}
                    className="text-xs text-text-tertiary hover:text-text-secondary transition-colors font-mono"
                  >
                    copy
                  </button>
                </div>
                <pre className="p-5 text-xs font-mono text-text-secondary overflow-x-auto leading-relaxed bg-background">
                  <code>{tab.request}</code>
                </pre>
              </div>

              {/* Response */}
              <div>
                <div className="flex items-center justify-between px-4 py-2 border-b border-border">
                  <span className="text-xs text-text-tertiary font-mono">Response <span className="text-accent">200</span></span>
                  <button
                    onClick={() => copy(tab.response)}
                    className="text-xs text-text-tertiary hover:text-text-secondary transition-colors font-mono"
                  >
                    copy
                  </button>
                </div>
                <pre className="p-5 text-xs font-mono text-text-secondary overflow-x-auto leading-relaxed bg-background">
                  <code>{tab.response}</code>
                </pre>
              </div>
            </motion.div>
          </AnimatePresence>
        </div>

        <div className="mt-6 flex items-center gap-4">
          <a
            href="https://compute.deep-ml.com/v1/health"
            target="_blank"
            rel="noreferrer"
            className="text-xs text-text-tertiary hover:text-accent transition-colors font-mono"
          >
            /v1/health ↗
          </a>
          <span className="text-border-strong">·</span>
          <a
            href="https://compute.deep-ml.com/llms.txt"
            target="_blank"
            rel="noreferrer"
            className="text-xs text-text-tertiary hover:text-accent transition-colors font-mono"
          >
            llms.txt ↗
          </a>
        </div>
      </div>
    </section>
  )
}
