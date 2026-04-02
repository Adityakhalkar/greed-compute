const BASE_URL = process.env.NEXT_PUBLIC_API_URL || 'https://compute.deep-ml.com'

async function request<T>(path: string, apiKey: string, options: RequestInit = {}): Promise<T> {
  const res = await fetch(`${BASE_URL}${path}`, {
    ...options,
    headers: {
      'Content-Type': 'application/json',
      'X-API-Key': apiKey,
      ...options.headers,
    },
  })
  if (!res.ok) {
    const body = await res.json().catch(() => ({}))
    throw new Error(body.error || `HTTP ${res.status}`)
  }
  return res.json()
}

export const api = {
  createSession: (apiKey: string, body: { template?: string }) =>
    request<{ session_id: string; template?: string }>('/v1/sessions', apiKey, {
      method: 'POST',
      body: JSON.stringify(body),
    }),

  execute: (apiKey: string, sessionId: string, code: string) =>
    request<{ stdout?: string; result?: unknown; error?: string }>(
      `/v1/sessions/${sessionId}/execute`,
      apiKey,
      { method: 'POST', body: JSON.stringify({ code }) }
    ),

  getSession: (apiKey: string, sessionId: string) =>
    request<{ session_id: string; created_at: number; last_active: number; template?: string }>(
      `/v1/sessions/${sessionId}`,
      apiKey
    ),

  deleteSession: (apiKey: string, sessionId: string) =>
    request<{ deleted: boolean }>(`/v1/sessions/${sessionId}`, apiKey, { method: 'DELETE' }),

  checkpoint: (apiKey: string, sessionId: string, name: string) =>
    request<{ checkpoint_id: string; name: string }>(
      `/v1/sessions/${sessionId}/checkpoint`,
      apiKey,
      { method: 'POST', body: JSON.stringify({ name }) }
    ),

  restoreCheckpoint: (apiKey: string, checkpointId: string) =>
    request<{ session_id: string }>('/v1/checkpoints/restore', apiKey, {
      method: 'POST',
      body: JSON.stringify({ checkpoint_id: checkpointId }),
    }),

  forkCheckpoint: (apiKey: string, checkpointId: string, count: number) =>
    request<{ session_ids: string[] }>('/v1/checkpoints/fork', apiKey, {
      method: 'POST',
      body: JSON.stringify({ checkpoint_id: checkpointId, count }),
    }),

  getUsage: (apiKey: string) =>
    request<{
      plan: string
      requests: { used: number; limit: number; remaining: number }
      storage: { used_mb: number; limit_mb: number; remaining_mb: number }
      checkpoint_retention_days: number
    }>('/v1/usage', apiKey),
}
