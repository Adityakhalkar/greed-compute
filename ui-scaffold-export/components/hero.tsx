'use client'

import { useEffect, useRef } from 'react'
import { motion } from 'framer-motion'
import { gsap } from 'gsap'
import Link from 'next/link'

const TERMINAL_LINES = [
  { delay: 0,    text: '$ python setup.py',            color: 'text-text-secondary' },
  { delay: 0.4,  text: '  importing torch... (12.3s)',  color: 'text-text-tertiary' },
  { delay: 0.8,  text: '  loading model... (8.1s)',     color: 'text-text-tertiary' },
  { delay: 1.2,  text: '  ready.',                      color: 'text-accent' },
  { delay: 1.6,  text: '$ greed checkpoint save base',  color: 'text-text-secondary' },
  { delay: 2.0,  text: '  checkpoint: a3f9c2b1',        color: 'text-accent' },
  { delay: 2.4,  text: '$ greed fork a3f9c2b1 --n 50',  color: 'text-text-secondary' },
  { delay: 2.8,  text: '  50 workers ready  (0.08s) ▋', color: 'text-accent' },
]

export function Hero() {
  const termRef = useRef<HTMLDivElement>(null)
  const linesRef = useRef<(HTMLParagraphElement | null)[]>([])

  useEffect(() => {
    linesRef.current.forEach((el, i) => {
      if (!el) return
      gsap.fromTo(el,
        { opacity: 0, y: 6 },
        { opacity: 1, y: 0, duration: 0.3, delay: TERMINAL_LINES[i].delay, ease: 'power2.out' }
      )
    })
  }, [])

  return (
    <section className="px-6 py-24 md:px-12 lg:px-24 border-b border-border">
      <div className="mx-auto max-w-6xl grid grid-cols-1 lg:grid-cols-2 gap-16 items-center">
        {/* Copy */}
        <div>
          <motion.p
            initial={{ opacity: 0, y: 8 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ duration: 0.4 }}
            className="text-xs tracking-widest uppercase text-text-tertiary mb-4 font-mono"
          >
            Stateful Python for AI agents
          </motion.p>
          <motion.h1
            initial={{ opacity: 0, y: 12 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ duration: 0.4, delay: 0.1 }}
            className="text-4xl md:text-5xl lg:text-6xl font-semibold text-text-primary leading-tight mb-6"
          >
            Python sessions<br />
            that <span className="text-accent">never</span> die.
          </motion.h1>
          <motion.p
            initial={{ opacity: 0, y: 12 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ duration: 0.4, delay: 0.2 }}
            className="text-text-secondary leading-relaxed mb-8 max-w-md"
          >
            Checkpoint interpreter state. Fork N parallel workers in milliseconds.
            Share sessions across any model. Zero cold starts.
          </motion.p>
          <motion.div
            initial={{ opacity: 0, y: 12 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ duration: 0.4, delay: 0.3 }}
            className="flex flex-col sm:flex-row gap-3"
          >
            <Link
              href="/playground"
              className="px-6 py-2.5 bg-accent text-background text-sm font-medium hover:bg-accent-dim transition-colors text-center"
            >
              Try playground →
            </Link>
            <a
              href="#api"
              className="px-6 py-2.5 border border-border text-text-secondary text-sm hover:border-border-strong hover:text-text-primary transition-colors text-center"
            >
              View API
            </a>
          </motion.div>
        </div>

        {/* Terminal */}
        <motion.div
          initial={{ opacity: 0, scale: 0.98 }}
          animate={{ opacity: 1, scale: 1 }}
          transition={{ duration: 0.5, delay: 0.2 }}
          className="bg-surface border border-border font-mono text-sm"
        >
          <div className="flex items-center gap-2 px-4 py-3 border-b border-border">
            <div className="w-2.5 h-2.5 rounded-full bg-border-strong" />
            <div className="w-2.5 h-2.5 rounded-full bg-border-strong" />
            <div className="w-2.5 h-2.5 rounded-full bg-border-strong" />
            <span className="ml-2 text-xs text-text-tertiary">greed-compute</span>
          </div>
          <div ref={termRef} className="p-5 space-y-1.5 min-h-[220px]">
            {TERMINAL_LINES.map((line, i) => (
              <p
                key={i}
                ref={el => { linesRef.current[i] = el }}
                className={`${line.color} opacity-0 text-xs leading-relaxed`}
              >
                {line.text}
              </p>
            ))}
          </div>
        </motion.div>
      </div>
    </section>
  )
}
