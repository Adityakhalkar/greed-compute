import type { Config } from 'tailwindcss'

const config: Config = {
  darkMode: 'class',
  content: [
    './pages/**/*.{ts,tsx}',
    './components/**/*.{ts,tsx}',
    './app/**/*.{ts,tsx}',
  ],
  theme: {
    extend: {
      colors: {
        background:      '#0A0A08',
        surface:         '#111110',
        'surface-raised':'#1A1A18',
        border:          '#252522',
        'border-strong': '#333330',
        'text-primary':  '#EFEFED',
        'text-secondary':'#888884',
        'text-tertiary': '#555552',
        accent:          '#C8F135',
        'accent-dim':    '#8AAD22',
        error:           '#E84040',
      },
      fontFamily: {
        sans: ['var(--font-geist-sans)', 'system-ui', 'sans-serif'],
        mono: ['var(--font-geist-mono)', 'monospace'],
      },
      borderRadius: {
        DEFAULT: '2px',
        sm:   '2px',
        md:   '4px',
        lg:   '6px',
        full: '9999px',
      },
      keyframes: {
        blink: {
          '0%, 100%': { opacity: '1' },
          '50%':      { opacity: '0' },
        },
      },
      animation: {
        'cursor-blink': 'blink 1s step-end infinite',
      },
    },
  },
  plugins: [],
}

export default config
