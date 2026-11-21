import assert from 'node:assert/strict'
import { once } from 'node:events'
import { mkdtemp, mkdir, rm, symlink, writeFile } from 'node:fs/promises'
import { request } from 'node:http'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { createWeb, readConfig } from '../server/http.mjs'
import { createSessions } from '../server/session.mjs'

const admin = 'test-admin-token-'.repeat(4)
const rust = 'test-api-token-'.repeat(4)
const control = 'test-control-token-'.repeat(4)
const origin = 'http://127.0.0.1:3000'
const config = readConfig({
  WEB_PUBLIC_ORIGIN: origin, WEB_ADMIN_TOKEN: admin,
  RUST_API_URL: 'http://api:8080', INTERNAL_API_TOKEN: rust,
  SIDECAR_CONTROL_URL: 'http://192.0.2.2:8090', SIDECAR_CONTROL_TOKEN: control,
})
const id = 'c2d502e3-18f7-413a-a379-9e6a71fb0bed'
const artifact = `/api/jobs/${id}/artifacts/0`
const state = {
  paired: false, connected: false, bot: '••••0102', owner_configured: true,
  pairing: { state: 'code', code: 'A1B2C3D4', expires_in: 180, error: null },
}
const ready = {
  id, kind: 'video', state: 'ready', next_index: 0, pending: false, sizes: [4], artifacts: [{ mime: 'video/mp4' }],
  error: null,
}

async function fixture(t, options = {}) {
  const temp = await mkdtemp(join(tmpdir(), 'vidora-web-'))
  const dist = join(temp, 'dist')
  const secret = join(temp, 'hidden.js')
  await mkdir(join(dist, 'assets'), { recursive: true })
  await writeFile(join(dist, 'index.html'), '<html><title>Vidora</title></html>')
  await writeFile(join(dist, 'assets', 'app-12345678.js'), 'void 0')
  await writeFile(secret, 'outside dist')
  await symlink(secret, join(dist, 'assets', 'escape.js'))
  const calls = []
  const fetcher = async (url, request) => {
    const { pathname, hostname } = new URL(url)
    const expected = hostname === 'api' ? rust : control
    assert.equal(request.headers.authorization, 'Bearer ' + expected)
    assert.equal(request.redirect, 'error')
    calls.push([pathname, request.method ?? 'GET'])
    const result = options.fetcher?.(pathname, request)
    if (result !== undefined) return result
    if (pathname === '/control/status') return Response.json(state)
    if (pathname === '/control/owner') return new Response(null, { status: 204 })
    if (pathname === '/control/pair') {
      return request.method === 'POST' ? Response.json({ accepted: true }, { status: 202 }) : new Response(null, { status: 204 })
    }
    if (pathname === '/internal/v1/jobs' || pathname === `/internal/v1/jobs/${id}`) return Response.json(ready)
    if (pathname === `/internal/v1/jobs/${id}/artifacts/0`) {
      return new Response(Buffer.from('test'), { headers: {
        'Content-Type': 'video/mp4', 'Content-Length': '4', 'Set-Cookie': 'upstream-secret',
        'Content-Disposition': 'inline', Server: 'external', 'X-Powered-By': 'external',
      } })
    }
    throw new Error('unexpected upstream')
  }
  const server = await createWeb({ config, dist, fetcher, now: options.now })
  server.listen(0, '127.0.0.1')
  await once(server, 'listening')
  t.after(async () => {
    const closed = new Promise((resolve) => server.close(resolve))
    server.closeAllConnections()
    await closed
    await rm(temp, { recursive: true, force: true })
  })
  const port = server.address().port
  const call = (path, { method = 'GET', body, headers = {} } = {}) => new Promise((resolve, reject) => {
    const options = { hostname: '127.0.0.1', port, path, method, headers: {
      host: '127.0.0.1:3000',
      ...(method === 'GET' ? {} : { origin, 'sec-fetch-site': 'same-origin' }),
      ...(body === undefined ? {} : { 'content-type': 'application/json' }), ...headers,
    } }
    request(options, (response) => {
      const chunks = []
      response.on('data', (chunk) => chunks.push(chunk))
      response.on('error', reject)
      response.on('end', () => resolve(new Response(response.statusCode === 204 ? null : Buffer.concat(chunks), {
        status: response.statusCode, headers: response.headers,
      })))
    }).on('error', reject).end(body === undefined ? undefined : JSON.stringify(body))
  })
  const login = async () => {
    const response = await call('/api/login', { method: 'POST', body: { token: admin } })
    assert.equal(response.status, 200)
    const cookie = response.headers.get('set-cookie')
    const { csrf } = await response.json()
    return { cookie, csrf, headers: { cookie: cookie.split(';')[0], 'x-csrf-token': csrf } }
  }
  return { call, calls, login, port }
}

test('public routes hide secrets, enforce Host and CSP, and serve only built static assets', async (t) => {
  const { call, calls, port } = await fixture(t)
  for (const path of ['/api/session', '/api/state', artifact]) {
    const response = await call(path)
    assert.equal(response.status, 401)
    assert.deepEqual(await response.json(), { error: 'unauthorized' })
  }
  assert.deepEqual(calls, [])
  const badHost = await call('/healthz', { headers: { host: 'outside.example' } })
  assert.equal(badHost.status, 421)
  assert.equal(badHost.headers.get('x-frame-options'), 'DENY')
  const health = await call('/healthz')
  assert.equal(health.status, 204)
  assert.equal(await health.text(), '')
  assert.equal(health.headers.get('cache-control'), 'no-store')
  const html = await call('/')
  assert.equal(html.status, 200)
  const csp = html.headers.get('content-security-policy')
  assert.match(csp, /script-src 'self'/)
  assert.match(csp, /frame-ancestors 'none'/)
  assert.doesNotMatch(csp, /unsafe-inline|unsafe-eval/)
  for (const header of ['x-content-type-options', 'referrer-policy', 'permissions-policy',
    'cross-origin-opener-policy', 'cross-origin-resource-policy']) assert.ok(html.headers.has(header))
  const js = await call('/assets/app-12345678.js')
  assert.equal(js.status, 200)
  assert.match(js.headers.get('cache-control'), /immutable/)
  const combined = (await html.text()) + (await js.text()) + JSON.stringify([...html.headers, ...js.headers])
  for (const value of [admin, rust, control, state.pairing.code]) assert.ok(!combined.includes(value))
  assert.equal((await call('/assets/escape.js')).status, 404)
  const traversal = await new Promise((resolve, reject) => {
    request({ hostname: '127.0.0.1', port, path: '/assets/%2e%2e/hidden.js', headers: { host: '127.0.0.1:3000' } },
      (response) => { response.resume(); resolve(response.statusCode) }).on('error', reject).end()
  })
  assert.equal(traversal, 404)
})

test('login throttles by socket IP and session cookie is random and protected', async (t) => {
  let now = 0
  const { call, login } = await fixture(t, { now: () => now })
  for (let index = 0; index < 5; index++) {
    const response = await call('/api/login', { method: 'POST', body: { token: 'wrong' }, headers: { 'x-forwarded-for': String(index) } })
    assert.equal(response.status, 401)
  }
  const throttled = await call('/api/login', { method: 'POST', body: { token: admin } })
  assert.equal(throttled.status, 429)
  assert.equal(throttled.headers.get('retry-after'), '300')
  now = 300_001
  const { cookie, csrf, headers } = await login()
  assert.match(cookie, /^vidora_session=[a-f0-9]{64}; HttpOnly; SameSite=Strict; Path=\/; Max-Age=43200$/)
  assert.match(csrf, /^[a-f0-9]{64}$/)
  assert.ok(!cookie.includes(csrf))
  for (const secret of [admin, rust, control]) assert.ok(!cookie.includes(secret))
  assert.deepEqual(await (await call('/api/session', { headers })).json(), { authenticated: true, csrf })
})

test('pairing and logout require CSRF, Origin and same-origin fetch metadata', async (t) => {
  const { call, login, calls } = await fixture(t)
  const { headers } = await login()
  for (const [path, method] of [['/api/pair', 'POST'], ['/api/pair', 'DELETE'], ['/api/logout', 'POST']]) {
    for (const badHeaders of [
      { cookie: headers.cookie },
      { ...headers, 'x-csrf-token': 'wrong' },
      { ...headers, origin: 'http://localhost:3000' },
      { ...headers, 'sec-fetch-site': 'same-site' },
    ]) assert.equal((await call(path, { method, headers: badHeaders })).status, 403)
  }
  assert.deepEqual(calls, [])
  assert.equal((await call('/api/pair', { method: 'POST', headers, body: { number: '12025550102' } })).status, 202)
  assert.equal((await call('/api/pair', { method: 'DELETE', headers })).status, 204)
  for (let index = 0; index < 4; index++) assert.equal((await call('/api/pair', { method: 'POST', headers, body: {} })).status, 202)
  assert.equal((await call('/api/pair', { method: 'POST', headers, body: {} })).status, 429)
  const logout = await call('/api/logout', { method: 'POST', headers })
  assert.equal(logout.status, 204)
  assert.match(logout.headers.get('set-cookie'), /Max-Age=0/)
  assert.equal((await call('/api/state', { headers })).status, 401)
})

test('state whitelists job and connection output and degrades either upstream independently', async (t) => {
  let offline
  const thumbnail = Buffer.from([255, 216, 255, 217]).toString('base64')
  const { call, login } = await fixture(t, {
    fetcher(path) {
      if (path === offline) return Promise.reject(new Error(admin))
      if (path === '/control/status') return Response.json({ ...state, number: '12025550102', leaked: control })
      if (path === '/internal/v1/jobs') return Response.json({
        ...ready, kind: 'photo', directory: '/private', error: '/another/path',
        artifacts: [{ mime: 'image/jpeg', width: 300, height: 200, thumbnail, path: '/secret' },
          { mime: '../secret', thumbnail: 'bm90IGpwZWc=' }],
      })
    },
  })
  const { headers } = await login()
  const response = await call('/api/state', { headers })
  assert.equal(response.status, 200)
  const value = await response.json()
  assert.deepEqual(value.connection, state)
  assert.deepEqual(value.job.artifacts, [{ mime: 'image/jpeg', width: 300, height: 200, thumbnail }, {}])
  assert.equal(value.job.error, 'job_failed')
  assert.doesNotMatch(JSON.stringify(value), /private|secret|12025550102|another\/path/)
  assert.equal(value.job_error, null)
  offline = '/control/status'
  const missingSidecar = await call('/api/state', { headers })
  assert.equal(missingSidecar.status, 200)
  assert.equal((await missingSidecar.json()).connection_reachable, false)
  offline = '/internal/v1/jobs'
  const missingJobs = await call('/api/state', { headers })
  assert.equal(missingJobs.status, 200)
  const degraded = await missingJobs.json()
  assert.equal(degraded.job, null)
  assert.equal(degraded.job_error, 'unavailable')
  assert.equal(degraded.connection_reachable, true)
})

test('owner setup only forwards an authenticated, CSRF-protected number', async (t) => {
  let submitted
  const { call, calls, login } = await fixture(t, {
    fetcher(path, request) {
      if (path === '/control/owner') submitted = JSON.parse(request.body)
    },
  })
  const body = { number: '+12025550101' }
  assert.equal((await call('/api/owner', { method: 'POST', body })).status, 401)
  const { headers } = await login()
  assert.equal((await call('/api/owner', { method: 'POST', body, headers: { cookie: headers.cookie } })).status, 403)
  assert.equal((await call('/api/owner', { method: 'POST', body: {}, headers })).status, 400)
  assert.equal((await call('/api/owner', { headers })).status, 404)
  assert.deepEqual(calls, [])
  const response = await call('/api/owner', { method: 'POST', body, headers })
  assert.equal(response.status, 204)
  assert.equal(await response.text(), '')
  assert.deepEqual(submitted, body)
  assert.deepEqual(calls, [['/control/owner', 'POST']])
})

test('only current artifact GETs reach the bounded proxy', async (t) => {
  let job = ready
  const { call, calls, login } = await fixture(t, {
    fetcher(path) { if (path === `/internal/v1/jobs/${id}`) return Response.json(job) },
  })
  const { headers } = await login()
  for (const [path, method] of [
    ['/api/jobs', 'POST'], [`/api/jobs/${id}/delivery`, 'POST'], [`/api/jobs/${id}/terminal`, 'POST'],
    [`/api/jobs/${id}/artifacts/32`, 'GET'], ['/api/jobs/not-a-uuid/artifacts/0', 'GET'],
    [`/api/jobs/${id}/artifacts/%2f0`, 'GET'], ['/api/unknown', 'GET'], [artifact, 'POST'],
  ]) assert.equal((await call(path, { method, headers })).status, 404)
  assert.deepEqual(calls, [])
  job = { ...ready, pending: true }
  assert.equal((await call(artifact, { headers })).status, 404)
  assert.deepEqual(calls, [[`/internal/v1/jobs/${id}`, 'GET']])
  job = ready
  const response = await call(artifact, { headers })
  assert.equal(response.status, 200)
  assert.equal(response.headers.get('content-type'), 'video/mp4')
  assert.equal(response.headers.get('content-length'), '4')
  assert.equal(response.headers.get('content-disposition'), 'attachment; filename="vidora-c2d502e3-0.mp4"')
  assert.equal(response.headers.get('cache-control'), 'no-store')
  for (const header of ['set-cookie', 'server', 'x-powered-by']) assert.ok(!response.headers.has(header))
  assert.equal(await response.text(), 'test')
  assert.equal(calls.at(-1)[1], 'GET')
})

test('oversized upstream JSON is withheld without leaking its body', async (t) => {
  const { call, login } = await fixture(t, {
    fetcher(path) {
      if (path === '/control/status') return Response.json({ ...state, junk: admin.repeat(300) })
      if (path === '/internal/v1/jobs') return Response.json({ ...ready, junk: rust.repeat(1500) })
    },
  })
  const { headers } = await login()
  const response = await call('/api/state', { headers })
  assert.equal(response.status, 200)
  assert.deepEqual(await response.json(), {
    connection: null, connection_reachable: false, job: null, job_error: 'unavailable',
  })
})

test('artifact upstream is aborted on client disconnect', async (t) => {
  let aborted
  const stop = new Promise((resolve) => { aborted = resolve })
  const { login, port } = await fixture(t, {
    fetcher(path, { signal }) {
      if (path !== `/internal/v1/jobs/${id}/artifacts/0`) return undefined
      signal.addEventListener('abort', aborted, { once: true })
      return new Response(new ReadableStream({
        start(controller) { controller.enqueue(Uint8Array.of(1)) },
      }), { headers: { 'content-type': 'video/mp4', 'content-length': '4' } })
    },
  })
  const { headers } = await login()
  await new Promise((resolve, reject) => {
    const incoming = request({ hostname: '127.0.0.1', port, path: artifact, headers: { ...headers, host: '127.0.0.1:3000' } },
      (response) => {
        assert.equal(response.statusCode, 200)
        response.once('data', () => { incoming.destroy(); resolve() })
      })
    incoming.setTimeout(1500, () => { incoming.destroy(); reject(new Error('artifact response timed out')) })
    incoming.on('error', reject).end()
  })
  assert.ok(await Promise.race([stop.then(() => true), new Promise((resolve) => setTimeout(resolve, 1000, false))]))
})

test('session limits evict the oldest, expire, and opt in to Secure', () => {
  let time = 0
  const sessions = createSessions({ ttl: 10, secure: true, now: () => time })
  const first = sessions.create()
  assert.match(first.cookie, /; Secure$/)
  for (let index = 0; index < 32; index++) sessions.create()
  assert.equal(sessions.get(first.cookie), undefined)
  const last = sessions.create()
  assert.equal(sessions.get(last.cookie).csrf, last.session.csrf)
  time = 10_000
  assert.equal(sessions.get(last.cookie), undefined)
})
