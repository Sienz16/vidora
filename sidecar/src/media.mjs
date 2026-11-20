import { createDecipheriv, createHmac, timingSafeEqual } from 'node:crypto'
import { Readable } from 'node:stream'
import { getMediaKeys } from '@whiskeysockets/baileys'

export function mediaUrl({ url, directPath }) {
  const locator = url ? new URL(url) : new URL('https://mmg.whatsapp.net')
  if (locator.protocol !== 'https:' || locator.username || locator.password || locator.port
    || !/^([\w-]+\.)+whatsapp\.net$/.test(locator.hostname)) throw new Error('invalid media origin')
  if (!directPath) {
    if (!url) throw new Error('missing media locator')
    return locator
  }
  if (typeof directPath !== 'string' || !/^\/(?![\\/])/.test(directPath)
    || /[\\\x00-\x20]/.test(directPath)) throw new Error('invalid media path')
  return new URL(directPath, locator.origin)
}

// Keep the last plaintext chunk until the media MAC has been checked.
export async function* decryptDocument(body, { cipherKey, iv, macKey }, bytes) {
  const encryptedSize = (Math.floor(bytes / 16) + 1) * 16
  const decipher = createDecipheriv('aes-256-cbc', cipherKey, iv)
  const mac = createHmac('sha256', macKey).update(iv)
  let received = 0
  let tail = Buffer.alloc(0)
  let last = Buffer.alloc(0)
  for await (const chunk of body) {
    received += chunk.length
    if (received > encryptedSize + 10) throw new Error('media length mismatch')
    const data = Buffer.concat([tail, chunk])
    if (data.length <= 10) {
      tail = data
      continue
    }
    const ciphertext = data.subarray(0, -10)
    tail = data.subarray(-10)
    mac.update(ciphertext)
    const clear = decipher.update(ciphertext)
    if (clear.length) {
      if (last.length) yield last
      last = clear
    }
  }
  if (received !== encryptedSize + 10 || !timingSafeEqual(tail, mac.digest().subarray(0, 10))) {
    throw new Error('media authentication failed')
  }
  const final = decipher.final()
  if (last.length) yield last
  if (final.length) yield final
}

export async function downloadMedia(message, bytes, mediaType, signal, reupload) {
  if (mediaType !== 'document' && mediaType !== 'image') {
    throw new Error('unsupported media type')
  }
  const downloadSignal = AbortSignal.any([signal, AbortSignal.timeout(80_000)])
  const extract = (msg) => (mediaType === 'image' ? msg.message?.imageMessage : msg.message?.documentMessage)
  let media = extract(message)
  if (!media) throw new Error('missing media message')
  const get = (data) => fetch(mediaUrl(data), {
    headers: { origin: 'https://web.whatsapp.com' },
    redirect: 'error',
    signal: downloadSignal,
  })
  let response = await get(media)
  if ([404, 410].includes(response.status)) {
    await response.body?.cancel()
    downloadSignal.throwIfAborted()
    const refreshed = await reupload(message)
    media = extract(refreshed)
    if (!media) throw new Error('missing media message')
    response = await get(media)
  }
  if (!response.ok || !response.body) {
    await response.body?.cancel()
    throw new Error('media download failed')
  }
  try {
    if (!(media.mediaKey instanceof Uint8Array) || media.mediaKey.length !== 32) {
      throw new Error('invalid media key')
    }
    const keys = await getMediaKeys(media.mediaKey, mediaType)
    return Readable.from(decryptDocument(Readable.fromWeb(response.body), keys, bytes), {
      objectMode: false,
      signal: downloadSignal,
    })
  } catch (error) {
    await response.body.cancel()
    throw error
  }
}

export const downloadDocument = (message, bytes, signal, reupload) =>
  downloadMedia(message, bytes, 'document', signal, reupload)
