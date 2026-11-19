import assert from 'node:assert/strict'
import { Readable } from 'node:stream'
import test from 'node:test'
import { ApiError } from '../src/api.mjs'
import { createBot, phoneJid } from '../src/bot.mjs'

const owner = phoneJid('12025550101')
const stranger = phoneJid('12025550102')
const photo = { mimetype: 'image/jpeg', fileLength: 3 }
const document = {
  mimetype: 'video/mp4', fileLength: 3, fileName: 'sample.mp4', fileSha256: Buffer.alloc(32),
}
const message = (id, jid, content = { documentMessage: document }, key = {}) => ({
  key: { id, remoteJid: jid, fromMe: false, ...key }, message: content,
})
const incoming = (...messages) => ({ type: 'notify', messages })

function fixtures() {
  const sent = []
  const notices = []
  const jobs = new Map()
  const counters = { downloads: 0, uploads: 0, prepares: 0 }
  let current
  const api = {
    status: async (_signal, id) => id ? jobs.get(id) : current,
    async upload(_source, metadata) {
      counters.uploads++
      current = { id: metadata.id, state: 'processing', next_index: 0, pending: false, sizes: [3, 3] }
      jobs.set(metadata.id, current)
      return { created: true, job: current }
    },
    artifact: async (_id, index) => Readable.from([Buffer.from(String(index))]),
    async delivery(id, index, action) {
      assert.equal(current.id, id)
      assert.equal(current.next_index, index)
      if (action === 'begin') {
        assert.equal(current.pending, false)
        current.pending = true
        current.state = 'delivering'
      } else {
        assert.equal(current.pending, true)
        current.pending = false
        current.next_index++
        if (current.next_index === current.sizes.length) current.state = 'delivered'
      }
      return current
    },
    async terminal(id, error) {
      assert.equal(current.id, id)
      current.state = 'failed'
      current.error = error
    },
  }
  const controller = new AbortController()
  const transport = (signal = new AbortController().signal) => ({
    signal,
    lookupLid: async (jid) => jid === '222@lid' ? owner : undefined,
    async download() {
      counters.downloads++
      return Readable.from([Buffer.from('mp4')])
    },
    async prepare(stream, index, total) {
      counters.prepares++
      for await (const bytes of stream) assert.equal(String(bytes), String(index))
      return { index, total }
    },
    async send(jid, item) {
      assert.equal(jid, owner)
      sent.push(item)
    },
    async reply(jid, text) {
      assert.equal(jid, owner)
      notices.push(text)
    },
  })
  return {
    controller, api, transport, sent, counters, notices,
    bot: () => createBot({ api, owner, stop: controller.signal }),
    job: () => current,
  }
}

test('only authenticated owner direct messages and supported documents start jobs', async () => {
  const { bot, transport, counters, notices } = fixtures()
  const wa = transport()
  const worker = bot()
  await worker.onUpsert(incoming(
    message('stranger', stranger, undefined, { remoteJidAlt: owner }),
    message('group', '12025550101@g.us', undefined, { participant: owner }),
    message('broadcast', 'status@broadcast', undefined, { remoteJidAlt: owner }),
    message('me', owner, undefined, { fromMe: true }),
    message('conflict', '222@lid', undefined, { remoteJidAlt: stranger }),
    message('unknown', '333@lid'),
    message('forwarded', owner, { extendedTextMessage: { text: '/help', contextInfo: { isForwarded: true } } }),
    message('not-video', owner, { documentMessage: photo }),
    message('large', owner, { documentMessage: { ...document, fileLength: 314572801 } }),
    message('bad-hash', owner, { documentMessage: { ...document, fileSha256: Buffer.alloc(8) } }),
  ), wa)
  await worker.onUpsert({ type: 'append', messages: [message('historical', owner)] }, wa)
  assert.equal(counters.uploads, 0)
  assert.equal(counters.downloads, 0)
  assert.equal(notices.length, 3)
  await worker.onUpsert(incoming(
    message('help', owner, { conversation: '/help' }),
    message('status', owner, { conversation: '/status' }),
    message('good', '222@lid'),
  ), wa)
  assert.match(notices[3], /Send a video/)
  assert.match(notices[4], /No video job/)
  assert.match(notices[5], /Video received/)
  assert.equal(counters.uploads, 1)
})

test('status describes a processing failure', async () => {
  const { bot, transport, job, notices } = fixtures()
  const worker = bot()
  const wa = transport()
  await worker.onUpsert(incoming(message('job', owner)), wa)
  job().state = 'failed'
  job().error = 'processing_failed'
  await worker.onUpsert(incoming(message('status', owner, { conversation: '/status' })), wa)
  assert.match(notices.at(-1), /couldn’t process that video/)
})

test('deduplicates message replay across sidecar instances and reports busy', async () => {
  const { bot, transport, counters, notices } = fixtures()
  const wa = transport()
  const event = incoming(message('same-job', owner))
  const worker = bot()
  await Promise.all([worker.onUpsert(event, wa), worker.onUpsert(event, wa)])
  await bot().onUpsert(event, wa)
  await worker.onUpsert(incoming(message('new-job', owner)), wa)
  assert.equal(counters.uploads, 1)
  assert.equal(counters.downloads, 1)
  assert.equal(notices.filter((text) => text.includes('Video received')).length, 1)
  assert.match(notices.at(-1), /already in progress/)
})

test('retries safe preparation and ACK failures without sending a clip twice', async () => {
  const { bot, transport, api, job, sent, counters } = fixtures()
  const worker = bot()
  const wa = transport()
  await worker.onUpsert(incoming(message('job', owner)), wa)
  job().state = 'ready'
  let failedPrepare = false
  const prepare = wa.prepare
  wa.prepare = async (...args) => {
    if (!failedPrepare) {
      failedPrepare = true
      throw new Error('transient upload failure')
    }
    return prepare(...args)
  }
  let failedAck = false
  const delivery = api.delivery
  api.delivery = async (id, index, action) => {
    if (action === 'ack' && !failedAck) {
      failedAck = true
      throw new ApiError(503)
    }
    return delivery(id, index, action)
  }
  await worker.tick(wa)
  await worker.tick(transport())
  assert.deepEqual(sent, [{ index: 0, total: 2 }, { index: 1, total: 2 }])
  assert.equal(counters.prepares, 2)
  assert.equal(job().state, 'delivered')
})

test('reconnect resumes after a disconnect before relay and never replays an ambiguous send', async () => {
  const { bot, transport, job, api, sent, notices } = fixtures()
  const worker = bot()
  const partition = new AbortController()
  const wa = transport(partition.signal)
  await worker.onUpsert(incoming(message('job', owner)), wa)
  job().state = 'ready'
  wa.prepare = async () => {
    partition.abort()
    throw new Error('disconnect during media staging')
  }
  await worker.tick(wa)
  assert.equal(job().pending, false)
  await worker.tick(transport())
  assert.equal(sent.length, 1)
  assert.equal(job().next_index, 1)
  await api.delivery(job().id, 1, 'begin')
  const restarted = bot()
  await restarted.tick(transport())
  await restarted.tick(transport())
  assert.equal(sent.length, 1)
  assert.equal(job().error, 'delivery_uncertain')
  assert.match(notices.at(-1), /could not be confirmed/)
})

test('a failure after relay starts never replays the same part', async () => {
  const { bot, transport, job, sent } = fixtures()
  const worker = bot()
  const wa = transport()
  await worker.onUpsert(incoming(message('job', owner)), wa)
  job().state = 'ready'
  const send = wa.send
  wa.send = async (...args) => {
    await send(...args)
    throw new Error('disconnected before server response')
  }
  await worker.tick(wa)
  await worker.tick(transport())
  assert.deepEqual(sent, [{ index: 0, total: 2 }])
  assert.equal(job().error, 'delivery_uncertain')
})
