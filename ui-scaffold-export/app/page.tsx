import { Nav } from '@/components/nav'
import { Hero } from '@/components/hero'
import { Stats } from '@/components/stats'
import { Primitives } from '@/components/primitives'
import { ApiReference } from '@/components/api-reference'
import { Pricing } from '@/components/pricing'
import { Footer } from '@/components/footer'

export default function Home() {
  return (
    <main>
      <Nav />
      <Hero />
      <Stats />
      {/* ILLUSTRATION ZONE — Problem section */}
      <section className="border-t border-border px-6 py-24 md:px-12 lg:px-24">
        <div className="mx-auto max-w-6xl grid grid-cols-1 md:grid-cols-2 gap-16 items-center">
          <div>
            <p className="text-xs tracking-widest uppercase text-text-tertiary mb-4">The problem</p>
            <h2 className="text-3xl md:text-4xl font-semibold text-text-primary leading-tight mb-6">
              Every agent starts<br />from scratch.
            </h2>
            <p className="text-text-secondary leading-relaxed mb-4">
              Cold imports. Model reloads. Redundant setup. Every time an agent
              needs Python, it waits — 5, 10, 30 seconds before any real work.
            </p>
            <p className="text-text-secondary leading-relaxed">
              Multiply that by a swarm of 50 agents. Multiply by every run.
              That's not infrastructure. That's waste.
            </p>
          </div>
          {/* ILLUSTRATION: agent stuck waiting — replace with Figma SVG export */}
          <div className="border border-dashed border-border-strong rounded-md aspect-square flex items-center justify-center">
            <p className="text-text-tertiary text-sm font-mono">illustration: cold-start-hell.svg</p>
          </div>
        </div>
      </section>

      {/* ILLUSTRATION ZONE — CALF section */}
      <section className="border-t border-border px-6 py-24 md:px-12 lg:px-24 bg-surface">
        <div className="mx-auto max-w-6xl grid grid-cols-1 md:grid-cols-2 gap-16 items-center">
          {/* ILLUSTRATION: fork diagram — replace with Figma SVG export */}
          <div className="border border-dashed border-border-strong rounded-md aspect-square flex items-center justify-center order-last md:order-first">
            <p className="text-text-tertiary text-sm font-mono">illustration: calf-fork.svg</p>
          </div>
          <div>
            <p className="text-xs tracking-widest uppercase text-text-tertiary mb-4">CALF — Checkpoint-based Agent Logical Fork</p>
            <h2 className="text-3xl md:text-4xl font-semibold text-text-primary leading-tight mb-6">
              Fork Python<br />like git.
            </h2>
            <p className="text-text-secondary leading-relaxed mb-6">
              Run your setup once. Checkpoint the interpreter state.
              Fork N workers — each starts with torch already imported,
              model already loaded, data already prepared.
            </p>
            <div className="flex items-baseline gap-3">
              <span className="text-5xl font-semibold text-accent tabular-nums">500×</span>
              <span className="text-text-secondary">faster than cold-spawning workers</span>
            </div>
          </div>
        </div>
      </section>

      <Primitives />
      <ApiReference />
      <Pricing />
      <Footer />
    </main>
  )
}
