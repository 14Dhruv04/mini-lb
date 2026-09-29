import React, { useCallback, useEffect, useMemo, useState } from 'react'
import { createRoot } from 'react-dom/client'
import './styles.css'

const INSTANCE_ID = 'dev-lb'

async function request(path, options = {}) {
  const response = await fetch(path, {
    ...options,
    headers: {
      ...(options.body ? { 'Content-Type': 'application/json' } : {}),
      ...(options.headers || {})
    }
  })

  const text = await response.text()
  let data = null
  try {
    data = text ? JSON.parse(text) : null
  } catch {
    data = text
  }

  if (!response.ok) {
    const message = typeof data === 'object' && data?.message ? data.message : `${response.status} ${response.statusText}`
    throw new Error(message)
  }
  return data
}

function normalizeInstance(raw) {
  const config = raw?.config || raw?.configuration || raw || {}
  return {
    raw,
    instanceId: raw?.instanceId || raw?.instance_id || INSTANCE_ID,
    name: raw?.name || raw?.instanceName || INSTANCE_ID,
    version: raw?.version ?? raw?.activeVersion ?? raw?.active_version ?? 0,
    config: {
      health_check_interval_seconds: config?.health_check_interval_seconds ?? 5,
      health_check_path: config?.health_check_path || '/health',
      backends: Array.isArray(config?.backends) ? config.backends.map((b) => ({
        id: b.id,
        url: b.url,
        enabled: b.enabled !== false
      })) : []
    }
  }
}

function normalizeEvents(raw) {
  return Array.isArray(raw?.events) ? raw.events : Array.isArray(raw) ? raw : []
}

function formatTime(timestampMs) {
  if (!timestampMs) return '—'
  const date = new Date(Number(timestampMs))
  return Number.isNaN(date.getTime()) ? '—' : date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' })
}

function formatLatency(value) {
  if (value === null || value === undefined) return '—'
  return `${Number(value).toFixed(0)} ms`
}

function percent(part, total) {
  if (!total) return 0
  return Math.round((part / total) * 100)
}

function App() {
  const [instance, setInstance] = useState(null)
  const [localStatus, setLocalStatus] = useState(null)
  const [events, setEvents] = useState([])
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  const [notice, setNotice] = useState('')
  const [lastRefresh, setLastRefresh] = useState(null)

  const refresh = useCallback(async (showSpinner = false) => {
    if (showSpinner) setLoading(true)
    try {
      setError('')
      const [instanceRaw, eventsRaw, localRaw] = await Promise.all([
        request(`/control/instances/${INSTANCE_ID}`),
        request(`/control/instances/${INSTANCE_ID}/events?size=50&type=request`),
        request('/local/status')
      ])
      setInstance(normalizeInstance(instanceRaw))
      setEvents(normalizeEvents(eventsRaw))
      setLocalStatus(localRaw)
      setLastRefresh(new Date())
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      if (showSpinner) setLoading(false)
    }
  }, [])

  useEffect(() => {
    refresh(true)
    const id = setInterval(() => refresh(false), 5000)
    return () => clearInterval(id)
  }, [refresh])

  const requestEvents = useMemo(
    () => events.filter((event) => event.event_type === 'request'),
    [events]
  )

  const stats = useMemo(() => {
    const successful = requestEvents.filter((e) => Number(e.status) >= 200 && Number(e.status) < 400).length
    const errors = requestEvents.filter((e) => Number(e.status) >= 500).length
    const latencies = requestEvents.map((e) => Number(e.latency_ms)).filter(Number.isFinite)
    const avg = latencies.length ? latencies.reduce((a, b) => a + b, 0) / latencies.length : 0
    return {
      total: requestEvents.length,
      successful,
      errors,
      avgLatency: avg,
      errorRate: percent(errors, requestEvents.length)
    }
  }, [requestEvents])

  const distribution = useMemo(() => {
    const counts = {}
    requestEvents.forEach((event) => {
      if (event.backend) counts[event.backend] = (counts[event.backend] || 0) + 1
    })
    const total = Object.values(counts).reduce((a, b) => a + b, 0)
    return Object.entries(counts)
      .sort((a, b) => b[1] - a[1])
      .map(([backend, count]) => ({ backend, count, percentage: percent(count, total) }))
  }, [requestEvents])

  const updateBackendEnabled = (backendId, enabled) => {
    setInstance((current) => current ? {
      ...current,
      config: {
        ...current.config,
        backends: current.config.backends.map((backend) =>
          backend.id === backendId ? { ...backend, enabled } : backend
        )
      }
    } : current)
  }

  const waitForActiveVersion = async (targetVersion) => {
    if (!targetVersion) return false

    const maxAttempts = 30
    const delayMs = 2000

    for (let attempt = 0; attempt < maxAttempts; attempt += 1) {
      const raw = await request(`/control/instances/${INSTANCE_ID}`)
      const current = normalizeInstance(raw)

      if (Number(current.version) >= Number(targetVersion)) {
        return true
      }

      await new Promise((resolve) => setTimeout(resolve, delayMs))
    }

    return false
  }

  const saveConfiguration = async () => {
    if (!instance) return
    setSaving(true)
    setNotice('')
    setError('')
    try {
      const result = await request(`/control/instances/${INSTANCE_ID}/config`, {
        method: 'PUT',
        body: JSON.stringify(instance.config)
      })

      const targetVersion = Number(result?.version) || null
      setNotice(
        targetVersion
          ? `Configuration update accepted. Waiting for version ${targetVersion} to become active…`
          : 'Configuration update accepted. Waiting for deployment to become active…'
      )

      const activated = await waitForActiveVersion(targetVersion)

      if (targetVersion && !activated) {
        setNotice(`Deployment is still in progress. Version ${targetVersion} has not become active yet.`)
      } else if (activated) {
        setNotice(`Configuration version ${targetVersion} is now active.`)
      }

      await refresh(false)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSaving(false)
    }
  }

  const activeBackends = localStatus?.backends?.filter((b) => b.enabled && b.healthy).length ?? 0
  const totalBackends = localStatus?.backends?.length ?? instance?.config.backends.length ?? 0
  const configVersion = instance?.version ?? '—'
  const healthLabel = totalBackends === 0 ? 'UNKNOWN' : activeBackends === totalBackends ? 'HEALTHY' : activeBackends > 0 ? 'DEGRADED' : 'DOWN'

  if (loading && !instance) {
    return <div className="shell"><div className="loading">Loading MiniLB Control Center…</div></div>
  }

  return (
    <div className="shell">
      <header className="topbar">
        <div>
          <div className="eyebrow">MINILB CONTROL CENTER</div>
          <h1>{instance?.name || INSTANCE_ID}</h1>
          <p className="muted">Instance <code>{INSTANCE_ID}</code></p>
        </div>
        <div className="header-actions">
          <span className={`pill ${healthLabel.toLowerCase()}`}><span className="dot" /> {healthLabel}</span>
          <button className="secondary" onClick={() => refresh(true)}>Refresh</button>
        </div>
      </header>

      {error && <div className="banner error">{error}</div>}
      {notice && <div className="banner success">{notice}</div>}

      <main>
        <section className="stat-grid">
          <StatCard label="Active backends" value={`${activeBackends}/${totalBackends}`} hint="Live Rust data plane" />
          <StatCard label="Config version" value={String(configVersion)} hint="DynamoDB active version" />
          <StatCard label="Requests sampled" value={String(stats.total)} hint="Latest 50 request events" />
          <StatCard label="Avg latency" value={formatLatency(stats.avgLatency)} hint={`${stats.errorRate}% 5xx rate`} />
        </section>

        <section className="grid-two">
          <Panel title="Backends" subtitle="Configuration from the cloud control plane">
            <div className="table">
              <div className="table-row table-head">
                <span>Backend</span><span>Endpoint</span><span>Health</span><span>Routing</span>
              </div>
              {(instance?.config.backends || []).map((backend) => {
                const live = localStatus?.backends?.find((item) => item.id === backend.id)
                return (
                  <div className="table-row" key={backend.id}>
                    <span className="strong">{backend.id}</span>
                    <span className="mono">{backend.url}</span>
                    <span><HealthBadge healthy={live?.healthy} /></span>
                    <label className="switch">
                      <input
                        type="checkbox"
                        checked={backend.enabled}
                        onChange={(event) => updateBackendEnabled(backend.id, event.target.checked)}
                      />
                      <span className="slider" />
                      <span className="switch-label">{backend.enabled ? 'Enabled' : 'Disabled'}</span>
                    </label>
                  </div>
                )
              })}
            </div>
            <div className="panel-footer">
              <span className="muted">Changes deploy through the Step Functions workflow.</span>
              <button className="primary" disabled={saving || !instance} onClick={saveConfiguration}>
                {saving ? 'Deploying…' : 'Deploy configuration'}
              </button>
            </div>
          </Panel>

          <Panel title="Traffic distribution" subtitle="Recent request events indexed in OpenSearch">
            {distribution.length === 0 ? (
              <div className="empty">No request telemetry yet.</div>
            ) : (
              <div className="distribution">
                {distribution.map((item) => (
                  <div className="dist-row" key={item.backend}>
                    <div className="dist-label"><span>{item.backend}</span><span>{item.percentage}%</span></div>
                    <div className="bar"><div className="bar-fill" style={{ width: `${item.percentage}%` }} /></div>
                    <div className="muted small">{item.count} requests</div>
                  </div>
                ))}
              </div>
            )}
          </Panel>
        </section>

        <section className="grid-two">
          <Panel title="Recent requests" subtitle="OpenSearch telemetry">
            <div className="table events">
              <div className="table-row table-head">
                <span>Time</span><span>Backend</span><span>Path</span><span>Status</span><span>Latency</span>
              </div>
              {requestEvents.slice(0, 12).map((event, index) => (
                <div className="table-row" key={event.eventId || `${event.request_id}-${index}`}>
                  <span className="mono small">{formatTime(event.timestamp_ms)}</span>
                  <span>{event.backend || '—'}</span>
                  <span className="mono">{event.path || '—'}</span>
                  <span><StatusBadge status={event.status} /></span>
                  <span>{formatLatency(event.latency_ms)}</span>
                </div>
              ))}
            </div>
          </Panel>

          <Panel title="Local data plane" subtitle="Read directly from MiniLB :9090">
            <div className="local-grid">
              <Info label="Strategy" value={localStatus?.strategy || '—'} />
              <Info label="Health path" value={localStatus?.health_check_path || '—'} />
              <Info label="Health interval" value={localStatus ? `${localStatus.health_check_interval_seconds}s` : '—'} />
              <Info label="Config source" value={localStatus?.config_path || '—'} mono />
            </div>
          </Panel>
        </section>

        <footer className="footer">
          <span>Auto-refresh every 5 seconds</span>
          <span>Last refresh: {lastRefresh ? lastRefresh.toLocaleTimeString() : '—'}</span>
        </footer>
      </main>
    </div>
  )
}

function StatCard({ label, value, hint }) {
  return <div className="stat-card"><div className="muted">{label}</div><div className="stat-value">{value}</div><div className="small muted">{hint}</div></div>
}

function Panel({ title, subtitle, children }) {
  return <section className="panel"><div className="panel-title"><div><h2>{title}</h2><p>{subtitle}</p></div></div>{children}</section>
}

function HealthBadge({ healthy }) {
  if (healthy === true) return <span className="status healthy"><span className="dot" /> Healthy</span>
  if (healthy === false) return <span className="status down"><span className="dot" /> Down</span>
  return <span className="status unknown">Unknown</span>
}

function StatusBadge({ status }) {
  const value = Number(status)
  const kind = value >= 500 ? 'down' : value >= 400 ? 'warn' : 'healthy'
  return <span className={`status ${kind}`}>{status ?? '—'}</span>
}

function Info({ label, value, mono = false }) {
  return <div className="info"><div className="muted small">{label}</div><div className={mono ? 'mono' : 'strong'}>{value}</div></div>
}

createRoot(document.getElementById('root')).render(<App />)
