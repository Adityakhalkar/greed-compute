'use client'

import { useEffect, useRef } from 'react'
import { gsap } from 'gsap'
import { ScrollTrigger } from 'gsap/ScrollTrigger'

gsap.registerPlugin(ScrollTrigger)

const PRIMITIVES = [
  {
    name: 'Sessions',
    endpoint: 'POST /v1/sessions',
    description: 'Create a persistent Python interpreter. Variables, imports, and state survive between executions.',
    code: `session = greed.create_session(template="data-science")
session.execute("import pandas as pd; df = pd.read_csv('data.csv')")
# df lives in memory — next call has it`,
  },
  {
    name: 'Checkpoints',
    endpoint: 'POST /v1/sessions/{id}/checkpoint',
    description: 'Snapshot the full interpreter state to durable storage. Restore or fork anytime.',
    code: `# Run expensive setup once
session.execute("model = load_model('llama-70b')")

# Save the warm state
ckpt = session.checkpoint(name="model-loaded")
# → checkpoint_id: "a3f9c2b1"`,
  },
  {
    name: 'Fork',
    endpoint: 'POST /v1/checkpoints/fork',
    description: 'Spawn N independent worker sessions from one checkpoint. Each gets a full copy — no re-initialization.',
    code: `# 50 workers with model pre-loaded, in 80ms
workers = greed.fork(checkpoint_id="a3f9c2b1", count=50)

for w in workers:
    w.execute(f"process_batch({w.index})")`,
  },
  {
    name: 'Execute',
    endpoint: 'POST /v1/sessions/{id}/execute',
    description: 'Run arbitrary Python in a session. Returns stdout, the expression result, and any errors.',
    code: `result = session.execute("""
import numpy as np
arr = np.random.randn(1000)
arr.mean()
""")
# result.result → -0.023
# result.stdout → ""`,
  },
]

export function Primitives() {
  const rootRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!rootRef.current) return
    const cards = rootRef.current.querySelectorAll('[data-card]')
    cards.forEach(card => {
      gsap.fromTo(card,
        { opacity: 0, y: 24 },
        {
          opacity: 1, y: 0, duration: 0.5, ease: 'power2.out',
          scrollTrigger: { trigger: card, start: 'top 85%' },
        }
      )
    })
  }, [])

  return (
    <section ref={rootRef} className="border-t border-border px-6 py-24 md:px-12 lg:px-24">
      <div className="mx-auto max-w-6xl">
        <div className="mb-12">
          <p className="text-xs tracking-widest uppercase text-text-tertiary mb-3 font-mono">Primitives</p>
          <h2 className="text-3xl md:text-4xl font-semibold text-text-primary">Four operations.<br />Infinite scale.</h2>
        </div>

        <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
          {PRIMITIVES.map(p => (
            <div key={p.name} data-card className="opacity-0 border border-border bg-surface p-6 flex flex-col gap-4">
              <div>
                <div className="flex items-baseline justify-between mb-2">
                  <p className="text-sm font-semibold text-text-primary">{p.name}</p>
                  <code className="text-xs text-text-tertiary font-mono">{p.endpoint}</code>
                </div>
                <p className="text-sm text-text-secondary leading-relaxed">{p.description}</p>
              </div>
              <pre className="bg-background border border-border p-4 text-xs font-mono text-text-secondary overflow-x-auto leading-relaxed">
                <code>{p.code}</code>
              </pre>
            </div>
          ))}
        </div>
      </div>
    </section>
  )
}
