'use client'

import { useState, useRef, useEffect } from 'react'
import { Nav } from '@/components/nav'
import { api } from '@/lib/api'

interface OutputLine {
  type: 'stdout' | 'result' | 'error' | 'system'
  content: string
}

const STARTER_CODE = `# greed-compute playground
# Your session persists between runs — variables stay in memory

import sys
print(f"Python {sys.version}")
`

export default function Playground() {
  const [code, setCode] = useState(STARTER_CODE)
  const [output, setOutput] = useState<OutputLine[]>([])
  const [sessionId, setSessionId] = useState<string | null>(null)
  const [apiKey, setApiKey] = useState<string>('')
  const [running, setRunning] = useState(false)
  const [template, setTemplate] = useState<string>('blank')
  const outputRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    const stored = localStorage.getItem('greed_api_key')
    if (stored) setApiKey(stored)
  }, [])

  useEffect(() => {
    if (outputRef.current) {
      outputRef.current.scrollTop = outputRef.current.scrollHeight
    }
  }, [output])

  const addOutput = (line: OutputLine) => {
    setOutput(prev => [...prev, line])
  }

  const createSession = async () => {
    if (!apiKey) {
      addOutput({ type: 'error', content: 'Set your API key first' })
      return
    }
    addOutput({ type: 'system', content: `Creating ${template} session...` })
    try {
      const session = await api.createSession(apiKey, { template })
      setSessionId(session.session_id)
      addOutput({ type: 'system', content: `Session ready: ${session.session_id.slice(0, 8)}... (template: ${session.template || 'blank'})` })
    } catch (e) {
      addOutput({ type: 'error', content: `Failed to create session: ${e}` })
    }
  }

  const runCode = async () => {
    if (!apiKey) { addOutput({ type: 'error', content: 'Enter API key first' }); return }
    let sid = sessionId
    if (!sid) {
      addOutput({ type: 'system', content: 'No session — creating one...' })
      try {
        const session = await api.createSession(apiKey, { template })
        sid = session.session_id
        setSessionId(sid)
        addOutput({ type: 'system', content: `Session: ${sid.slice(0, 8)}...` })
      } catch (e) {
        addOutput({ type: 'error', content: `${e}` }); return
      }
    }

    setRunning(true)
    addOutput({ type: 'system', content: `Running...` })
    try {
      const result = await api.execute(apiKey, sid, code)
      if (result.stdout) {
        result.stdout.split('\n').filter(Boolean).forEach((line: string) => {
          addOutput({ type: 'stdout', content: line })
        })
      }
      if (result.result !== null && result.result !== undefined) {
        addOutput({ type: 'result', content: JSON.stringify(result.result) })
      }
      if (result.error) {
        addOutput({ type: 'error', content: result.error })
      }
    } catch (e) {
      addOutput({ type: 'error', content: `${e}` })
    } finally {
      setRunning(false)
    }
  }

  const clearOutput = () => setOutput([])
  const newSession = () => { setSessionId(null); addOutput({ type: 'system', content: 'Session cleared. Next run creates a fresh session.' }) }

  return (
    <main className="h-screen flex flex-col">
      <Nav />
      <div className="flex-1 flex flex-col overflow-hidden">
        {/* Toolbar */}
        <div className="border-b border-border bg-surface px-4 py-2 flex items-center gap-3">
          <select
            value={template}
            onChange={e => setTemplate(e.target.value)}
            className="bg-background border border-border px-2 py-1 text-xs font-mono text-text-secondary focus:outline-none focus:border-accent"
          >
            <option value="blank">blank</option>
            <option value="data-science">data-science</option>
            <option value="machine-learning">machine-learning</option>
            <option value="web-scraping">web-scraping</option>
          </select>

          <input
            type="password"
            placeholder="API key: gc-..."
            value={apiKey}
            onChange={e => {
              setApiKey(e.target.value)
              localStorage.setItem('greed_api_key', e.target.value)
            }}
            className="flex-1 max-w-xs bg-background border border-border px-2 py-1 text-xs font-mono text-text-primary placeholder:text-text-tertiary focus:outline-none focus:border-accent"
          />

          <div className="flex items-center gap-1 ml-auto">
            {sessionId && (
              <span className="text-xs font-mono text-text-tertiary mr-2">
                {sessionId.slice(0, 8)}...
              </span>
            )}
            <button onClick={newSession} className="px-3 py-1 text-xs border border-border text-text-secondary hover:border-border-strong hover:text-text-primary transition-colors">
              New session
            </button>
            <button onClick={clearOutput} className="px-3 py-1 text-xs border border-border text-text-secondary hover:border-border-strong hover:text-text-primary transition-colors">
              Clear
            </button>
            <button
              onClick={runCode}
              disabled={running}
              className="px-4 py-1 text-xs bg-accent text-background font-medium hover:bg-accent-dim disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
            >
              {running ? 'Running...' : 'Run ▶'}
            </button>
          </div>
        </div>

        {/* Editor + Output split */}
        <div className="flex-1 grid grid-cols-2 overflow-hidden">
          {/* Editor */}
          <div className="border-r border-border flex flex-col">
            <div className="px-3 py-1.5 border-b border-border">
              <span className="text-xs text-text-tertiary font-mono">editor</span>
            </div>
            <textarea
              value={code}
              onChange={e => setCode(e.target.value)}
              onKeyDown={e => {
                if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
                  e.preventDefault()
                  runCode()
                }
                if (e.key === 'Tab') {
                  e.preventDefault()
                  const start = e.currentTarget.selectionStart
                  const end = e.currentTarget.selectionEnd
                  const newCode = code.substring(0, start) + '    ' + code.substring(end)
                  setCode(newCode)
                  setTimeout(() => e.currentTarget.setSelectionRange(start + 4, start + 4), 0)
                }
              }}
              className="flex-1 bg-background text-text-primary font-mono text-sm p-4 resize-none focus:outline-none leading-relaxed"
              spellCheck={false}
            />
            <div className="px-3 py-1.5 border-t border-border text-xs text-text-tertiary font-mono">
              Ctrl+Enter to run
            </div>
          </div>

          {/* Output */}
          <div className="flex flex-col">
            <div className="px-3 py-1.5 border-b border-border">
              <span className="text-xs text-text-tertiary font-mono">output</span>
            </div>
            <div ref={outputRef} className="flex-1 overflow-y-auto p-4 font-mono text-sm space-y-1">
              {output.length === 0 && (
                <p className="text-text-tertiary">Run code to see output here.</p>
              )}
              {output.map((line, i) => (
                <p key={i} className={
                  line.type === 'error'  ? 'text-error' :
                  line.type === 'result' ? 'text-accent' :
                  line.type === 'system' ? 'text-text-tertiary' :
                  'text-text-primary'
                }>
                  {line.type === 'result' && <span className="text-text-tertiary">→ </span>}
                  {line.type === 'system' && <span className="text-text-tertiary"># </span>}
                  {line.content}
                </p>
              ))}
              {running && (
                <p className="text-text-tertiary animate-pulse">▋</p>
              )}
            </div>
          </div>
        </div>
      </div>
    </main>
  )
}
