import { FormEvent, ReactNode, useEffect, useState } from 'react'
import { createRoot } from 'react-dom/client'
import { Database, KeyRound, LogOut, Plus, RefreshCw, Save, ServerCog, Trash2 } from 'lucide-react'
import './styles.css'

type Adapter = {
  id: number
  name: string
  kind: string
  api_key: string
  enabled: boolean
  priority: number
  default_model: string
  opus_model: string
  sonnet_model: string
  haiku_model: string
  thinking: string | null
  reasoning_effort: string | null
}

type ClientKey = {
  id: number
  name: string
  api_key: string
  enabled: boolean
  priority: number
}

type AdminState = {
  adapters: Adapter[]
  client_keys: ClientKey[]
}

const emptyAdapter: Omit<Adapter, 'id'> = {
  name: 'DeepSeek',
  kind: 'deepseek',
  api_key: '',
  enabled: true,
  priority: 100,
  default_model: 'deepseek-v4-flash',
  opus_model: 'deepseek-v4-pro',
  sonnet_model: 'deepseek-v4-flash',
  haiku_model: 'deepseek-v4-flash',
  thinking: 'auto',
  reasoning_effort: 'high',
}

const emptyKey = {
  name: 'default',
  api_key: '',
  enabled: true,
  priority: 100,
}

function App() {
  const [authenticated, setAuthenticated] = useState(false)
  const [username, setUsername] = useState('')
  const [password, setPassword] = useState('')
  const [state, setState] = useState<AdminState | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    void checkSession()
  }, [])

  async function checkSession() {
    const response = await fetch('/api/admin/session')
    const body = await response.json()
    setAuthenticated(body.authenticated)
    if (body.authenticated) {
      await loadState()
    }
  }

  async function loadState() {
    setError('')
    const response = await fetch('/api/admin/state')
    if (!response.ok) {
      setAuthenticated(false)
      return
    }
    setState(await response.json())
  }

  async function login(event: FormEvent) {
    event.preventDefault()
    setBusy(true)
    setError('')
    const response = await fetch('/api/admin/login', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ username, password }),
    })
    setBusy(false)
    if (!response.ok) {
      setError('登录失败')
      return
    }
    setAuthenticated(true)
    await loadState()
  }

  async function logout() {
    await fetch('/api/admin/logout', { method: 'POST' })
    setAuthenticated(false)
    setState(null)
  }

  if (!authenticated) {
    return (
      <main className="login-shell">
        <form className="login-panel" onSubmit={login}>
          <div className="brand">
            <ServerCog size={28} />
            <div>
              <h1>deepseed2claude</h1>
              <p>Admin</p>
            </div>
          </div>
          <label>
            用户名
            <input value={username} onChange={(event) => setUsername(event.target.value)} />
          </label>
          <label>
            密码
            <input
              type="password"
              value={password}
              onChange={(event) => setPassword(event.target.value)}
            />
          </label>
          {error ? <div className="error">{error}</div> : null}
          <button className="primary" disabled={busy}>
            <KeyRound size={16} />
            登录
          </button>
        </form>
      </main>
    )
  }

  return (
    <main className="app-shell">
      <header>
        <div>
          <h1>Gateway Admin</h1>
          <p>Adapters and Claude Code client keys</p>
        </div>
        <div className="actions">
          <button onClick={loadState}>
            <RefreshCw size={16} />
            刷新
          </button>
          <button onClick={logout}>
            <LogOut size={16} />
            退出
          </button>
        </div>
      </header>

      {state ? (
        <>
          <Adapters adapters={state.adapters} reload={loadState} />
          <ClientKeys keys={state.client_keys} reload={loadState} />
        </>
      ) : (
        <section className="empty">Loading</section>
      )}
    </main>
  )
}

function Adapters({ adapters, reload }: { adapters: Adapter[]; reload: () => Promise<void> }) {
  const [draft, setDraft] = useState(emptyAdapter)

  async function createAdapter(event: FormEvent) {
    event.preventDefault()
    await jsonRequest('/api/admin/adapters', 'POST', draft)
    setDraft(emptyAdapter)
    await reload()
  }

  return (
    <section>
      <SectionTitle icon={<Database size={18} />} title="Adapters" />
      <form className="grid-form" onSubmit={createAdapter}>
        <AdapterFields adapter={draft} onChange={setDraft} />
        <button className="primary">
          <Plus size={16} />
          新建 adapter
        </button>
      </form>

      <div className="table">
        {adapters.map((adapter) => (
          <AdapterRow key={adapter.id} adapter={adapter} reload={reload} />
        ))}
      </div>
    </section>
  )
}

function AdapterRow({ adapter, reload }: { adapter: Adapter; reload: () => Promise<void> }) {
  const [draft, setDraft] = useState(adapter)

  async function saveAdapter() {
    await jsonRequest(`/api/admin/adapters/${adapter.id}`, 'PUT', draft)
    await reload()
  }

  async function deleteAdapter() {
    await fetch(`/api/admin/adapters/${adapter.id}`, { method: 'DELETE' })
    await reload()
  }

  return (
    <article className="row-card">
      <div className="row-head">
        <strong>{adapter.name}</strong>
        <span>{adapter.enabled ? 'enabled' : 'disabled'}</span>
      </div>
      <div className="grid-form compact">
        <AdapterFields adapter={draft} onChange={setDraft} />
        <div className="row-actions">
          <button onClick={saveAdapter}>
            <Save size={16} />
            保存
          </button>
          <button className="danger" onClick={deleteAdapter}>
            <Trash2 size={16} />
            删除
          </button>
        </div>
      </div>
    </article>
  )
}

function ClientKeys({ keys, reload }: { keys: ClientKey[]; reload: () => Promise<void> }) {
  const [draft, setDraft] = useState({ ...emptyKey, name: 'claude-code' })
  async function create(event: FormEvent) {
    event.preventDefault()
    await jsonRequest('/api/admin/client-keys', 'POST', draft)
    setDraft({ ...emptyKey, name: 'claude-code' })
    await reload()
  }
  return (
    <section>
      <SectionTitle icon={<KeyRound size={18} />} title="Claude Code client keys" />
      <form className="key-form" onSubmit={create}>
        <KeyFields value={draft} onChange={setDraft} />
        <button className="primary">
          <Plus size={16} />
          新建调用 key
        </button>
      </form>
      <div className="key-list">
        {keys.map((key) => (
          <ClientKeyRow key={key.id} item={key} reload={reload} />
        ))}
      </div>
    </section>
  )
}

function ClientKeyRow({ item, reload }: { item: ClientKey; reload: () => Promise<void> }) {
  const [draft, setDraft] = useState(item)
  async function save() {
    await jsonRequest(`/api/admin/client-keys/${item.id}`, 'PUT', draft)
    await reload()
  }
  async function remove() {
    await fetch(`/api/admin/client-keys/${item.id}`, { method: 'DELETE' })
    await reload()
  }
  return (
    <div className="inline-row">
      <KeyFields value={draft} onChange={setDraft} />
      <button onClick={save}>
        <Save size={16} />
      </button>
      <button className="danger" onClick={remove}>
        <Trash2 size={16} />
      </button>
    </div>
  )
}

function AdapterFields<T extends Omit<Adapter, 'id'> | Adapter>({
  adapter,
  onChange,
}: {
  adapter: T
  onChange: (value: T) => void
}) {
  const set = (patch: Partial<Adapter>) => onChange({ ...adapter, ...patch })
  return (
    <>
      <input value={adapter.name} onChange={(event) => set({ name: event.target.value })} />
      <select value={adapter.kind} onChange={(event) => set({ kind: event.target.value })}>
        <option value="deepseek">deepseek</option>
      </select>
      <input
        className="key-input"
        value={adapter.api_key}
        onChange={(event) => set({ api_key: event.target.value })}
      />
      <input
        type="number"
        value={adapter.priority}
        onChange={(event) => set({ priority: Number(event.target.value) })}
      />
      <label className="check">
        <input
          type="checkbox"
          checked={adapter.enabled}
          onChange={(event) => set({ enabled: event.target.checked })}
        />
        enabled
      </label>
      <input
        value={adapter.default_model}
        onChange={(event) => set({ default_model: event.target.value })}
      />
      <input value={adapter.opus_model} onChange={(event) => set({ opus_model: event.target.value })} />
      <input
        value={adapter.sonnet_model}
        onChange={(event) => set({ sonnet_model: event.target.value })}
      />
      <input value={adapter.haiku_model} onChange={(event) => set({ haiku_model: event.target.value })} />
      <select
        value={adapter.thinking ?? ''}
        onChange={(event) => set({ thinking: event.target.value || null })}
      >
        <option value="">inherit</option>
        <option value="auto">auto</option>
        <option value="disabled">disabled</option>
        <option value="enabled">enabled</option>
      </select>
      <input
        value={adapter.reasoning_effort ?? ''}
        onChange={(event) => set({ reasoning_effort: event.target.value || null })}
      />
    </>
  )
}

function KeyFields<T extends Omit<ClientKey, 'id'> | ClientKey>({
  value,
  onChange,
}: {
  value: T
  onChange: (value: T) => void
}) {
  const set = (patch: Partial<ClientKey>) => onChange({ ...value, ...patch })
  return (
    <>
      <input value={value.name} onChange={(event) => set({ name: event.target.value })} />
      <input
        className="key-input"
        value={value.api_key}
        onChange={(event) => set({ api_key: event.target.value })}
      />
      <input
        type="number"
        value={value.priority}
        onChange={(event) => set({ priority: Number(event.target.value) })}
      />
      <label className="check">
        <input
          type="checkbox"
          checked={value.enabled}
          onChange={(event) => set({ enabled: event.target.checked })}
        />
        enabled
      </label>
    </>
  )
}

function SectionTitle({ icon, title }: { icon: ReactNode; title: string }) {
  return (
    <div className="section-title">
      {icon}
      <h2>{title}</h2>
    </div>
  )
}

async function jsonRequest(path: string, method: string, body: unknown) {
  const response = await fetch(path, {
    method,
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  })
  if (!response.ok) {
    throw new Error(await response.text())
  }
}

createRoot(document.getElementById('root')!).render(<App />)
