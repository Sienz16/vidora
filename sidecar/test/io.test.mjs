import assert from 'node:assert/strict'
import { once } from 'node:events'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { createServer } from 'node:http'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { Readable } from 'node:stream'
import { buffer } from 'node:stream/consumers'
import test from 'node:test'
import { encryptedStream, getMediaKeys } from '@whiskeysockets/baileys'
import { createApi, MAX_INPUT } from '../src/api.mjs'
import { openAuth } from '../src/auth.mjs'
import { decryptDocument, mediaUrl } from '../src/media.mjs'

test('session SQLite persists Signal keys and refuses a second writer', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'vidora-auth-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const first = openAuth(directory)
  const key = Buffer.from('test-key')
  try {
    assert.throws(() => openAuth(directory))
    await first.state.keys.set({ session: { 'fake-id': key } })
    first.state.creds.registered = true
    first.save()
  } finally {
    first.close()
  }
  const reopened = openAuth(directory)
  try {
    assert.equal(reopened.state.creds.registered, true)
    assert.deepEqual(await reopened.state.keys.get('session', ['fake-id']), { 'fake-id': key })
    await reopened.state.keys.set({ session: { 'fake-id': null } })
    assert.equal((await reopened.state.keys.get('session', ['fake-id']))['fake-id'], null)
  } finally {
    reopened.close()
  }
})

test('media download limits and authenticates Baileys-produced document bytes', async () => {
  const clear = Buffer.from('media payload with a known document HMAC')
  const encrypted = await encryptedStream({ stream: Readable.from([clear]) }, 'document')
  try {
    const keys = await getMediaKeys(encrypted.mediaKey, 'document')
    const bytes = await readFile(encrypted.encFilePath)
    const pieces = [...bytes].map((byte) => Buffer.from([byte]))
    assert.deepEqual(await buffer(Readable.from(decryptDocument(pieces, keys, clear.length))), clear)
    const incorrect = Buffer.from(bytes)
    incorrect[incorrect.length - 1] ^= 1
    await assert.rejects(buffer(Readable.from(decryptDocument([incorrect], keys, clear.length))), /authentication/)
    await assert.rejects(buffer(Readable.from(decryptDocument([bytes.subarray(0, -1)], keys, clear.length))), /authentication/)
    await assert.rejects(buffer(Readable.from(decryptDocument([Buffer.concat([bytes, bytes])], keys, clear.length))), /length/)
  } finally {
    await rm(encrypted.encFilePath, { force: true })
  }
  assert.equal(mediaUrl({ directPath: '/v/test?data=x' }).hostname, 'mmg.whatsapp.net')
  assert.throws(() => mediaUrl({ url: 'https://mmg.whatsapp.net.local/v/test' }))
  assert.throws(() => mediaUrl({ url: 'https://mmg.whatsapp.net@localhost/v/test' }))
  assert.throws(() => mediaUrl({ directPath: '//localhost/test' }))
  assert.throws(() => mediaUrl({ directPath: '/\\localhost/test' }))
})

test('HTTP client streams bounded uploads and downloads against a mock Rust API', async (t) => {
  const received = []
  const token = 'fake-auth-token-for-test-only'
  const server = createServer(async (request, response) => {
    assert.equal(request.headers.authorization, `Bearer ${token}`)
    if (request.method === 'POST') {
      assert.equal(request.headers['x-message-id'], 'job')
      try {
        received.push(await buffer(request))
      } catch {
        return
      }
      response.writeHead(202, { 'content-type': 'application/json' })
      response.end('{"state":"processing"}')
    } else if (request.url.endsWith('/0')) {
      response.writeHead(200, { 'content-type': 'video/mp4', 'content-length': '3' })
      response.end('mp4')
    } else {
      response.writeHead(200, { 'content-type': 'video/mp4', 'content-length': '5' })
      response.end('other')
    }
  })
  server.listen(0, '127.0.0.1')
  await once(server, 'listening')
  t.after(() => { server.closeAllConnections(); server.close() })
  const api = createApi(`http://127.0.0.1:${server.address().port}`, token)
  const signal = new AbortController().signal
  const data = { id: 'job', bytes: 3, digest: '0'.repeat(64) }
  const result = await api.upload(Readable.from([Buffer.from('m'), Buffer.from('p4')]), data, signal)
  assert.deepEqual(result, { created: true, job: { state: 'processing' } })
  assert.deepEqual(received, [Buffer.from('mp4')])
  await assert.rejects(api.upload(Readable.from([Buffer.from('too long')]), data, signal))
  await assert.rejects(api.upload(Readable.from([Buffer.from('m')]), data, signal))
  await assert.rejects(api.upload(Readable.from([]), { ...data, bytes: MAX_INPUT + 1 }, signal), /size/)
  assert.deepEqual(await buffer(await api.artifact('job', 0, 3, signal)), Buffer.from('mp4'))
  await assert.rejects(api.artifact('job', 1, 3, signal))

  const interrupted = new AbortController()
  const source = new Readable({ read() {} })
  const transfer = api.upload(source, data, interrupted.signal)
  interrupted.abort()
  await assert.rejects(transfer)
  assert.equal(source.destroyed, true)
})
