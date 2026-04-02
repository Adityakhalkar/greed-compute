import type { Metadata } from 'next'
import { GeistSans } from 'geist/font/sans'
import { GeistMono } from 'geist/font/mono'
import { Toaster } from 'sonner'
import './globals.css'

export const metadata: Metadata = {
  title: 'greed-compute — Stateful Python for AI agents',
  description: 'Checkpoint interpreter state, fork N parallel workers, share across any model. Zero cold starts.',
  openGraph: {
    title: 'greed-compute',
    description: 'Stateful Python for AI agents. 500× faster warm starts.',
    url: 'https://compute.deep-ml.com',
  },
}

export default function RootLayout({
  children,
}: {
  children: React.ReactNode
}) {
  return (
    <html lang="en" className={`${GeistSans.variable} ${GeistMono.variable}`}>
      <body className="bg-background text-text-primary font-sans">
        {children}
        <Toaster
          theme="dark"
          toastOptions={{
            style: {
              background: 'var(--surface)',
              border: '1px solid var(--border)',
              color: 'var(--text-primary)',
              borderRadius: '2px',
            },
          }}
        />
      </body>
    </html>
  )
}
