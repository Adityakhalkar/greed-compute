'use client'

import { motion } from 'framer-motion'
import { cn } from '@/lib/utils'

const PLANS = [
  {
    name: 'Free',
    price: '$0',
    period: 'forever',
    description: 'Get started. No credit card.',
    features: [
      '100 executions / day',
      '3 sessions max',
      '50 MB checkpoint storage',
      '1 day retention',
      'Blank template only',
      'Community support',
    ],
    cta: 'Get API key',
    href: 'mailto:hello@deep-ml.com?subject=greed-compute free key',
    accent: false,
  },
  {
    name: 'Pro',
    price: '$29',
    period: '/ month',
    description: 'For builders shipping production agents.',
    features: [
      'Unlimited executions',
      '50 sessions max',
      '5 GB checkpoint storage',
      '30 day retention',
      'All templates (ML, data-science, scraping)',
      'Fork up to 50 workers',
      'Email support',
    ],
    cta: 'Upgrade to Pro',
    href: '/upgrade',
    accent: true,
  },
  {
    name: 'Enterprise',
    price: 'Custom',
    period: '',
    description: 'Dedicated infra, SLAs, volume pricing.',
    features: [
      'Unlimited everything',
      'Dedicated warm pool',
      'Custom templates',
      'SSO / SAML',
      'SLA 99.9%',
      'Slack / dedicated support',
    ],
    cta: 'Talk to us',
    href: 'mailto:hello@deep-ml.com?subject=greed-compute enterprise',
    accent: false,
  },
]

export function Pricing() {
  return (
    <section id="pricing" className="border-t border-border px-6 py-24 md:px-12 lg:px-24">
      <div className="mx-auto max-w-6xl">
        <div className="mb-12">
          <p className="text-xs tracking-widest uppercase text-text-tertiary mb-3 font-mono">Pricing</p>
          <h2 className="text-3xl md:text-4xl font-semibold text-text-primary">Simple. No surprises.</h2>
        </div>

        <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
          {PLANS.map((plan, i) => (
            <motion.div
              key={plan.name}
              initial={{ opacity: 0, y: 20 }}
              whileInView={{ opacity: 1, y: 0 }}
              viewport={{ once: true }}
              transition={{ duration: 0.4, delay: i * 0.1 }}
              className={cn(
                'border p-6 flex flex-col',
                plan.accent ? 'border-accent bg-surface' : 'border-border bg-surface'
              )}
            >
              {plan.accent && (
                <div className="text-xs font-mono text-accent mb-4 tracking-widest uppercase">Most popular</div>
              )}

              <div className="mb-6">
                <p className="text-sm text-text-tertiary mb-1">{plan.name}</p>
                <div className="flex items-baseline gap-1 mb-2">
                  <span className="text-3xl font-semibold text-text-primary tabular-nums">{plan.price}</span>
                  {plan.period && <span className="text-text-tertiary text-sm">{plan.period}</span>}
                </div>
                <p className="text-xs text-text-secondary">{plan.description}</p>
              </div>

              <ul className="space-y-2 mb-8 flex-1">
                {plan.features.map(f => (
                  <li key={f} className="flex items-start gap-2 text-xs text-text-secondary">
                    <span className="text-accent mt-0.5 shrink-0">·</span>
                    {f}
                  </li>
                ))}
              </ul>

              <a
                href={plan.href}
                className={cn(
                  'block text-center py-2.5 text-xs font-medium transition-colors',
                  plan.accent
                    ? 'bg-accent text-background hover:bg-accent-dim'
                    : 'border border-border text-text-secondary hover:border-border-strong hover:text-text-primary'
                )}
              >
                {plan.cta}
              </a>
            </motion.div>
          ))}
        </div>
      </div>
    </section>
  )
}
