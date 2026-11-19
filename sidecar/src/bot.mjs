import { setTimeout as delay } from 'node:timers/promises'
import { ApiError, MAX_INPUT } from './api.mjs'

const ACTIVE = new Set(['receiving', 'processing', 'ready', 'delivering'])
const HELP = 'Send a video as a document (up to 300 MiB and 10 minutes). I’ll return MP4 parts for you to post manually. Use /status to check the current job.'
const BUSY = 'A video is already in progress. Try again when it finishes.'
const UNCERTAIN = 'Delivery could not be confirmed. Check which parts reached this chat before resubmitting the video.'
const STATES = {
  receiving: 'Receiving your video.',
  processing: 'Processing your video.',
  ready: 'Your clips are ready to send.',
  delivering: 'Sending your clips.',
  delivered: 'All clips were sent.',
}

function describe(job) {
  if (job?.state !== 'failed') return STATES[job?.state] ?? 'No video job yet.'
  switch (job.error) {
    case 'delivery_uncertain': return UNCERTAIN
    case 'delivery_failed': return 'Your clips were prepared, but delivery failed.'
    case 'processing_failed': return 'I couldn’t process that video. Check that it is playable, SDR, and no longer than 10 minutes.'
    case 'expired': return 'Your clips expired before delivery. Send the video again to start a new job.'
    default: return 'The video job failed. Send the video again to retry.'
  }
}

export function phoneJid(number) {
  if (typeof number !== 'string' || !/^\+?[1-9][0-9]{6,14}$/.test(number)) {
    throw new Error('invalid WhatsApp number')
  }
  return `${number.replace(/^\+/, '')}@s.whatsapp.net`
}

function pn(jid) {
  const match = /^([1-9][0-9]{6,14})(?::[0-9]+)?@s\.whatsapp\.net$/.exec(jid ?? '')
  return match ? `${match[1]}@s.whatsapp.net` : null
}

async function ownerMessage(message, owner, wa) {
  const { key } = message
  if (!key || key.fromMe !== false || key.participant || key.participantAlt
    || typeof key.id !== 'string' || !/^[a-zA-Z\d_-]{1,128}$/.test(key.id)) return false
  const primary = pn(key.remoteJid)
  if (primary) return primary === owner
  if (!/^\d+(?::\d+)?@(?:hosted\.)?lid$/.test(key.remoteJid ?? '')) return false
  const alternate = pn(key.remoteJidAlt)
  if (alternate) return alternate === owner
  return pn(await wa.lookupLid(key.remoteJid)) === owner
}

function mediaInput(id, document) {
  const name = document.fileName
  const mime = document.mimetype
  if (typeof mime !== 'string' || mime.length > 100
    || (!/^video\/[\w.+-]+$/i.test(mime)
      && !(mime === 'application/octet-stream' && typeof name === 'string'
        && /\.(mp4|mov|mkv|webm|avi|mts|ts|m2ts|3gp)$/i.test(name)))) return null
  const length = String(document.fileLength ?? '')
  const bytes = Number(length)
  if (!/^[1-9]\d{0,8}$/.test(length) || bytes > MAX_INPUT
    || !(document.fileSha256 instanceof Uint8Array) || document.fileSha256.length !== 32) return null
  return {
    id,
    bytes,
    digest: Buffer.from(document.fileSha256).toString('hex'),
    name: typeof name === 'string' && /^[\x20-\x7e]{1,200}$/.test(name)
      && !/[\\/]/.test(name) ? name : undefined,
  }
}

async function retry(operation, signal) {
  for (let attempt = 0; ; attempt++) {
    signal.throwIfAborted()
    try {
      return await operation()
    } catch (error) {
      if (attempt === 2 || (error instanceof ApiError && error.status < 500
        && ![408, 429].includes(error.status))) throw error
      await delay(500 * (attempt + 1), undefined, { signal })
    }
  }
}

export function createBot({ api, owner, stop, log = () => {} }) {
  let running
  let receivingId
  let acknowledged
  let lastFailure
  let lastBusy = -Infinity

  const clear = () => { running = undefined }
  function exclusive(operation) {
    if (running) return running
    running = operation().catch(() => log('work_failed')).finally(clear)
    return running
  }

  async function reply(wa, text) {
    if (wa.signal.aborted) return false
    try {
      await wa.reply(owner, text)
      return true
    } catch {
      log('owner_notice_failed')
      return false
    }
  }

  async function busy(wa) {
    if (performance.now() - lastBusy < 10_000) return
    lastBusy = performance.now()
    await reply(wa, BUSY)
  }

  async function accept(wa, message, input) {
    try {
      const result = await retry(async () => {
        const existing = await api.status(wa.signal, input.id)
        if (existing?.state === 'receiving') throw new Error('upload still recovering')
        if (existing) return { created: false }
        if (ACTIVE.has((await api.status(wa.signal))?.state)) throw new ApiError(409)
        const source = await wa.download(message, input.bytes)
        try {
          return await api.upload(source, input, wa.signal)
        } finally {
          source.destroy()
        }
      }, wa.signal)
      if (result.created) await reply(wa, 'Video received. I’ll send the clips here when they’re ready.')
    } catch (error) {
      if (error instanceof ApiError && error.status === 409) await busy(wa)
      else await reply(wa, 'I couldn’t receive that video. Try sending it again as a document.')
    }
  }

  async function handle(message, wa) {
    if (!await ownerMessage(message, owner, wa) || wa.signal.aborted) return
    const content = message.message?.ephemeralMessage?.message ?? message.message
    const document = content?.documentMessage ?? content?.documentWithCaptionMessage?.message?.documentMessage
    const extended = content?.extendedTextMessage
    const command = content?.conversation ?? extended?.text
    const forwarded = extended?.contextInfo?.isForwarded || extended?.contextInfo?.forwardingScore
    if (!forwarded && command === '/help') {
      await reply(wa, HELP)
      return
    }
    if (!forwarded && command === '/status') {
      let job
      try {
        job = await api.status(wa.signal)
      } catch {
        await reply(wa, 'Status is temporarily unavailable.')
        return
      }
      await reply(wa, describe(job))
      return
    }
    if (!document) return
    const input = mediaInput(message.key.id, document)
    if (!input) {
      await reply(wa, 'Send a supported video document up to 300 MiB.')
      return
    }
    while (running) {
      if (receivingId === input.id || await api.status(wa.signal, input.id)) return
      if (receivingId || ACTIVE.has((await api.status(wa.signal))?.state)) return busy(wa)
      await running
    }
    receivingId = input.id
    await exclusive(async () => {
      try {
        await accept(wa, { ...message, message: { documentMessage: document } }, input)
      } finally {
        receivingId = undefined
      }
    })
  }

  async function ack(job, index, wa) {
    const result = await retry(() => api.delivery(job, index, 'ack', stop), stop)
    acknowledged = undefined
    if (result.state === 'delivered') await reply(wa, STATES.delivered)
  }

  async function deliver(job, wa) {
    const { id, next_index: index, pending, sizes } = job
    if (pending) {
      if (acknowledged?.id === id && acknowledged.index === index) await ack(id, index, wa)
      else await api.terminal(id, 'delivery_uncertain', stop)
      return
    }
    let prepared
    try {
      prepared = await retry(async () => {
        const clip = await api.artifact(id, index, sizes[index], wa.signal)
        try {
          return await wa.prepare(clip, index, sizes.length)
        } finally {
          clip.destroy()
        }
      }, wa.signal)
    } catch {
      if (!wa.signal.aborted) await api.terminal(id, 'delivery_failed', stop)
      return
    }
    wa.signal.throwIfAborted()
    await api.delivery(id, index, 'begin', stop)
    try {
      await wa.send(owner, prepared)
    } catch {
      await api.terminal(id, 'delivery_uncertain', stop)
      return
    }
    acknowledged = { id, index }
    await ack(id, index, wa)
  }

  return {
    async onUpsert(event, wa) {
      if (event.type !== 'notify' || wa.signal.aborted) return
      for (const message of event.messages) await handle(message, wa)
    },
    tick(wa) {
      if (running || wa.signal.aborted) return
      return exclusive(async () => {
        const job = await api.status(wa.signal)
        if (job?.state === 'ready' || job?.state === 'delivering') await deliver(job, wa)
        if (job?.state === 'failed' && job.id !== lastFailure
          && await reply(wa, describe(job))) {
          lastFailure = job.id
        }
      })
    },
    drained: () => running,
  }
}
