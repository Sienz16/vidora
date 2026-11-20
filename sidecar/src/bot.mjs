import { setTimeout as delay } from 'node:timers/promises'
import { ApiError, MAX_INPUT, MAX_PHOTO_INPUT } from './api.mjs'

const ACTIVE = new Set(['receiving', 'processing', 'ready', 'delivering'])
const HELP = 'Send a video as a document (up to 300 MiB and 10 minutes) or a photo (as a document or HD image, up to 50 MiB). I’ll return MP4 parts or a prepared HD JPEG for you to post or forward manually. Use /status to check the current job.'
const BUSY = 'A media job is already in progress. Try again when it finishes.'
const UNCERTAIN = 'Delivery could not be confirmed. Check which parts reached this chat before resubmitting the media.'
const STATES = {
  receiving: 'Receiving your media.',
  processing: 'Processing your media.',
  ready: 'Your output is ready to send.',
  delivering: 'Sending your output.',
  delivered: 'All output was sent.',
}

function describe(job) {
  const isPhoto = job?.kind === 'photo'
  if (job?.state !== 'failed') return STATES[job?.state] ?? 'No media job yet.'
  switch (job.error) {
    case 'delivery_uncertain': return UNCERTAIN
    case 'delivery_failed':
      return isPhoto ? 'Your HD photo was prepared, but delivery failed.' : 'Your clips were prepared, but delivery failed.'
    case 'processing_failed':
      return isPhoto
        ? 'I couldn’t process that photo. Check that it is a valid JPEG, PNG, WebP, HEIC, BMP, or TIFF image.'
        : 'I couldn’t process that video. Check that it is playable and no longer than 10 minutes.'
    case 'input_size':
      return isPhoto ? 'That photo exceeds the 50 MiB limit.' : 'That file exceeds the 300 MiB limit.'
    case 'unsupported_hdr':
      return 'I couldn’t convert this HDR format. HLG and HDR10 media with standard color tags are supported.'
    case 'expired':
      return 'Your prepared output expired before delivery. Send the media again to start a new job.'
    default: return 'The media job failed. Send the media again to retry.'
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

function validMeta(media, maxBytes) {
  const length = String(media.fileLength ?? '')
  const bytes = Number(length)
  if (!/^[1-9]\d{0,8}$/.test(length) || bytes > maxBytes
    || !(media.fileSha256 instanceof Uint8Array) || media.fileSha256.length !== 32) return null
  return { bytes, digest: Buffer.from(media.fileSha256).toString('hex') }
}

function mediaInput(id, content) {
  const document = content?.documentMessage ?? content?.documentWithCaptionMessage?.message?.documentMessage
  if (document) {
    const name = document.fileName
    const mime = document.mimetype
    if (typeof mime !== 'string' || mime.length > 100) return null
    const isVideo = /^video\/[\w.+-]+$/i.test(mime)
      || (mime === 'application/octet-stream' && typeof name === 'string'
        && /\.(mp4|mov|mkv|webm|avi|mts|ts|m2ts|3gp)$/i.test(name))
    const isPhoto = /^image\/(jpeg|jpg|png|webp|heic|heif|bmp|tiff)$/i.test(mime)
      || (mime === 'application/octet-stream' && typeof name === 'string'
        && /\.(jpe?g|png|webp|heic|heif|bmp|tiff?)$/i.test(name))
    if (!isVideo && !isPhoto) return null
    const kind = isPhoto ? 'photo' : 'video'
    const meta = validMeta(document, isPhoto ? MAX_PHOTO_INPUT : MAX_INPUT)
    if (!meta) return null
    return {
      id,
      ...meta,
      mediaType: 'document',
      kind,
      name: typeof name === 'string' && /^[\x20-\x7e]{1,200}$/.test(name)
        && !/[\\/]/.test(name) ? name : undefined,
    }
  }
  const image = content?.imageMessage
  if (image) {
    const mime = image.mimetype
    if (mime !== undefined && mime !== null
      && (typeof mime !== 'string' || mime.length > 100
        || !/^image\/(jpeg|jpg|png|webp|heic|heif)$/i.test(mime))) return null
    const meta = validMeta(image, MAX_PHOTO_INPUT)
    if (!meta) return null
    return { id, ...meta, mediaType: 'image', kind: 'photo' }
  }
  return null
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
        const source = await wa.download(message, input.bytes, input.mediaType)
        try {
          return await api.upload(source, input, wa.signal)
        } finally {
          source.destroy()
        }
      }, wa.signal)
      if (result.created) {
        await reply(
          wa,
          input.kind === 'photo'
            ? 'Photo received. I’ll send the HD image here when it’s ready—forward it directly to Status.'
            : 'Video received. I’ll send the clips here when they’re ready.',
        )
      }
    } catch (error) {
      if (error instanceof ApiError && error.status === 409) await busy(wa)
      else {
        await reply(
          wa,
          input.kind === 'photo'
            ? 'I couldn’t receive that photo. Try sending it again as a document or HD image.'
            : 'I couldn’t receive that video. Try sending it again as a document.',
        )
      }
    }
  }

  async function handle(message, wa) {
    if (!await ownerMessage(message, owner, wa) || wa.signal.aborted) return
    const content = message.message?.ephemeralMessage?.message ?? message.message
    const document = content?.documentMessage ?? content?.documentWithCaptionMessage?.message?.documentMessage
    const image = content?.imageMessage
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
    if (!document && !image) return
    const input = mediaInput(message.key.id, content)
    if (!input) {
      await reply(wa, 'Send a supported video document (up to 300 MiB) or photo (as a document or HD image, up to 50 MiB).')
      return
    }
    while (running) {
      if (receivingId === input.id || await api.status(wa.signal, input.id)) return
      if (receivingId || ACTIVE.has((await api.status(wa.signal))?.state)) return busy(wa)
      await running
    }
    const normalized = input.mediaType === 'image' ? { imageMessage: image } : { documentMessage: document }
    receivingId = input.id
    await exclusive(async () => {
      try {
        await accept(wa, { ...message, message: normalized }, input)
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
    const kind = job.kind === 'photo' ? 'photo' : 'video'
    const contentType = kind === 'photo' ? 'image/jpeg' : 'video/mp4'
    const meta = job.artifacts?.[index]
    let prepared
    try {
      prepared = await retry(async () => {
        const artifact = await api.artifact(id, index, sizes[index], wa.signal, contentType)
        try {
          return await wa.prepare(artifact, index, sizes.length, { kind, meta })
        } finally {
          artifact.destroy()
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
