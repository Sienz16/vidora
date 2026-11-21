import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { once } from 'node:events'
import { mkdtemp, rm } from 'node:fs/promises'
import { request } from 'node:http'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { DatabaseSync } from 'node:sqlite'
import { setTimeout as delay } from 'node:timers/promises'
import test from 'node:test'
import { openAuth } from '../src/auth.mjs'
import { controlConfig, createControl } from '../src/control.mjs'
import { createPairing, linkedBot } from '../src/pairing.mjs'

const owner = '12025550101@s.whatsapp.net'
const pinned = '12025550102@s.whatsapp.net'
const controlToken = 'test-only-control-token-'.repeat(2)
const fakeCode = 'A1B2C3D4'
const bearer = 'Bearer ' + controlToken

async function fixture(t, options = {}) {
  const directory = await mkdtemp(join(tmpdir(), 'vidora-control-'))
  const auth = openAuth(directory)
  const stop = new AbortController()
  const log = []
  const pairing = createPairing({
    auth, owner, pinned: options.pinned ?? null, stop: stop.signal,
    bot: { async drained() {} }, log: (event) => log.push(event), deadlineMs: options.deadlineMs,
    dial: options.dial ?? (async ({ auth: staged, stop: signal, pairing: show }) => {
      staged.state.creds.pairingCode = fakeCode
      staged.save()
      show(fakeCode)
      if (!signal.aborted) await once(signal, 'abort')
      return { reason: 'stopped' }
    }),
  })
  const server = createControl({ token: controlToken, pairing })
  server.listen(0, '127.0.0.1')
  await once(server, 'listening')
  t.after(async () => {
    stop.abort()
    await pairing.drain()
    const closed = new Promise((resolve) => server.close(resolve))
    server.closeAllConnections()
    await closed
    auth.close()
    await rm(directory, { recursive: true, force: true })
  })
  const base = `http://127.0.0.1:${server.address().port}`
  const call = async (method, path, body, headers = { authorization: bearer }) => {
    const response = await fetch(base + '/control' + path, {
      method, headers: { ...(body === undefined ? {} : { 'content-type': 'application/json' }), ...headers },
      ...(body === undefined ? {} : { body: typeof body === 'string' ? body : JSON.stringify(body) }),
    })
    return [response.status, response.status === 204 ? null : await response.json(), response.headers]
  }
  const status = async () => (await call('GET', '/status'))[1]
  return { auth, directory, pairing, log, call, status, port: server.address().port }
}

test('control requires a single exact bearer and returns bounded JSON only', async (t) => {
  const { call, port } = await fixture(t)
  for (const headers of [{}, { authorization: 'Basic ' + controlToken }, { authorization: 'Bearer ' + 'x'.repeat(32) }]) {
    assert.equal((await call('GET', '/status', undefined, headers))[0], 401)
  }
  const duplicate = await new Promise((resolve, reject) => {
    request({ hostname: '127.0.0.1', port, path: '/control/status', headers: { authorization: [bearer, bearer] } },
      (response) => { response.resume(); resolve(response.statusCode) }).on('error', reject).end()
  })
  assert.equal(duplicate, 401)
  const [status, , headers] = await call('PUT', '/pair')
  assert.equal(status, 405)
  assert.equal(headers.get('allow'), 'POST, DELETE')
  assert.equal(headers.get('cache-control'), 'no-store')
  assert.equal((await call('POST', '/pair', {}, { authorization: bearer, 'content-type': 'text/plain' }))[0], 400)
  assert.equal((await call('POST', '/pair', ' '.repeat(1025)))[0], 413)
  assert.equal((await call('POST', '/pair', { number: 42 }))[0], 400)
  assert.equal((await call('POST', '/pair', { extra: '' }))[0], 400)
})

test('unpinned pairing validates the number, issues one code, cancels and retries', async (t) => {
  const { call, status, pairing, log, directory } = await fixture(t)
  for (const number of ['1234', '+01234567', '123 4567', 'abcdefghi', '1234567890123456', '+12025550101']) {
    assert.deepEqual((await call('POST', '/pair', { number })).slice(0, 2), [400, { error: 'invalid_number' }])
  }
  assert.equal((await call('POST', '/pair', { number: '+12025550102' }))[0], 202)
  assert.equal((await status()).pairing.code, fakeCode)
  const db = new DatabaseSync(join(directory, 'session.db'), { readOnly: true })
  try {
    assert.deepEqual(db.prepare('SELECT value FROM entries').all(), [])
  } finally { db.close() }
  assert.deepEqual((await call('POST', '/pair', { number: '12025550102' })).slice(0, 2), [409, { error: 'busy' }])
  assert.equal((await call('DELETE', '/pair'))[0], 204)
  await pairing.drain()
  assert.deepEqual((await status()).pairing, { state: 'cancelled', code: null, error: null, expires_in: null })
  assert.equal((await call('POST', '/pair', { number: '12025550102' }))[0], 202)
  assert.ok(log.includes('pairing_code_issued'))
  assert.ok(!log.join(' ').includes(fakeCode))
})

test('a short pairing deadline hides the code and permits retry', async (t) => {
  const { pairing, call, status, log } = await fixture(t, { deadlineMs: 50 })
  assert.equal((await call('POST', '/pair', { number: '12025550102' }))[0], 202)
  await delay(80)
  await pairing.drain()
  assert.deepEqual((await status()).pairing, {
    state: 'expired', code: null, error: 'pairing_timed_out', expires_in: null,
  })
  assert.equal((await call('POST', '/pair', { number: '12025550102' }))[0], 202)
  assert.ok(log.includes('pairing_expired'))
})

test('verified pairing saves the linked identity and keys without the code', async (t) => {
  const { call, status, pairing, auth, directory, log } = await fixture(t, {
    pinned,
    dial: async ({ auth: staged, botJid, pairing: show }) => {
      assert.equal(botJid, pinned)
      show(fakeCode)
      staged.state.creds.me = { id: '12025550102:7@s.whatsapp.net' }
      staged.state.creds.pairingCode = fakeCode
      await staged.state.keys.set({ session: { fake: { valid: true } } })
      staged.state.creds.registered = true
      return { reason: 'paired' }
    },
  })
  assert.deepEqual((await call('POST', '/pair', { number: '+12025550103' })).slice(0, 2),
    [400, { error: 'number_mismatch' }])
  assert.equal((await call('POST', '/pair', {}))[0], 202)
  assert.equal(await pairing.waitForPair(), true)
  assert.equal(linkedBot(auth, owner, null), pinned)
  pairing.online(true)
  assert.deepEqual(await status(), {
    paired: true, connected: true, bot: '••••0102', owner_configured: true,
    pairing: { state: 'paired', code: null, error: null, expires_in: null },
  })
  pairing.online(false)
  assert.equal((await status()).connected, false)
  for (const method of ['POST', 'DELETE']) {
    assert.deepEqual((await call(method, '/pair', method === 'POST' ? {} : undefined)).slice(0, 2),
      [409, { error: 'already_paired' }])
  }
  assert.throws(() => auth.beginPairing())
  const db = new DatabaseSync(join(directory, 'session.db'), { readOnly: true })
  try {
    const rows = db.prepare('SELECT value FROM entries').all()
    assert.equal(rows.length, 2)
    assert.ok(!JSON.stringify(rows).includes(fakeCode))
  } finally { db.close() }
  assert.ok(!log.join(' ').includes(fakeCode))
})

test('wrong account discards only unregistered state and allows retry', async (t) => {
  let calls = 0
  const { auth, pairing, status, call } = await fixture(t, {
    dial: async ({ auth: staged, botJid }) => {
      staged.state.creds.registered = true
      staged.state.creds.me = { id: calls++ === 0 ? owner : botJid }
      return { reason: 'paired' }
    },
  })
  await auth.state.keys.set({ session: { partial: 'old' } })
  assert.equal((await call('POST', '/pair', { number: '+12025550102' }))[0], 202)
  await pairing.drain()
  assert.equal((await status()).pairing.state, 'wrong_account')
  assert.equal(auth.state.creds.registered, false)
  assert.deepEqual(await auth.state.keys.get('session', ['partial']), { partial: null })
  assert.equal((await call('POST', '/pair', { number: '+12025550102' }))[0], 202)
  assert.equal(await pairing.waitForPair(), true)
  assert.equal(linkedBot(auth, owner, null), pinned)
})

test('control configuration is opt-in and refuses wildcard or shared credentials', () => {
  assert.equal(controlConfig({}), null)
  assert.equal(controlConfig({ SIDECAR_CONTROL_TOKEN: controlToken }).host, '127.0.0.1')
  assert.throws(() => controlConfig({ SIDECAR_CONTROL_TOKEN: '' }))
  assert.throws(() => controlConfig({ SIDECAR_CONTROL_TOKEN: controlToken, SIDECAR_CONTROL_BIND: '0.0.0.0' }))
  assert.throws(() => controlConfig({ SIDECAR_CONTROL_TOKEN: controlToken, INTERNAL_API_TOKEN: controlToken }))
})

test('startup modes enforce the headless pin and keep unpinned UI mode alive', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'vidora-startup-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const env = {
    OWNER_WHATSAPP_NUMBER: '12025550101',
    BAILEYS_AUTH_DIR: directory,
    INTERNAL_API_TOKEN: 'test-only-api-token-'.repeat(3),
    RUST_API_URL: 'http://127.0.0.1:9',
  }
  const start = (args, config) => spawn(process.execPath, [new URL('../src/main.mjs', import.meta.url).pathname, ...args], {
    env: { ...env, ...config }, stdio: ['ignore', 'ignore', 'pipe'],
  })
  for (const [args, config, event] of [
    [[], {}, 'bot_number_required'],
    [['--pair'], {}, 'bot_number_required'],
    [[], { BOT_WHATSAPP_NUMBER: '12025550102' }, 'sidecar_pairing_required'],
  ]) {
    const child = start(args, config)
    const chunks = []
    child.stderr.on('data', (chunk) => chunks.push(chunk))
    assert.deepEqual(await once(child, 'close'), [1, null])
    assert.equal(Buffer.concat(chunks).toString().trim(), event)
  }
  const reserve = createControl({ token: controlToken, pairing: {} })
  reserve.listen(0, '127.0.0.1')
  await once(reserve, 'listening')
  const port = reserve.address().port
  await new Promise((resolve) => reserve.close(resolve))
  const child = start([], { SIDECAR_CONTROL_TOKEN: controlToken, SIDECAR_CONTROL_PORT: String(port) })
  t.after(() => { if (child.exitCode === null) child.kill() })
  const closed = once(child, 'close')
  let state
  for (let i = 0; i < 50; i++) {
    try {
      const response = await fetch(`http://127.0.0.1:${port}/control/status`, { headers: { authorization: bearer } })
      if (response.ok) { state = await response.json(); break }
    } catch {}
    await delay(40)
  }
  assert.equal(state?.paired, false)
  assert.equal(state.bot, null)
  child.kill('SIGTERM')
  assert.deepEqual(await closed, [0, null])
})
