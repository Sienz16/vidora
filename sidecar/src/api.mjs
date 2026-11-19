import { request } from 'node:http'
import { Transform, Readable } from 'node:stream'
import { pipeline } from 'node:stream/promises'

export const MAX_INPUT = 300 * 1024 * 1024
const MAX_CLIP = 15_800_000

export class ApiError extends Error {
  constructor(status) {
    super('api request failed')
    this.status = status
  }
}

function matchLength(expected) {
  let received = 0
  return new Transform({
    transform(chunk, _encoding, callback) {
      received += chunk.length
      if (received > expected) return callback(new Error('size limit'))
      callback(null, chunk)
    },
    flush(callback) {
      callback(received === expected ? undefined : new Error('size mismatch'))
    },
  })
}

async function readJson(body) {
  const chunks = []
  let size = 0
  for await (const chunk of body) {
    size += chunk.length
    if (size > 16_384) throw new Error('api response too large')
    chunks.push(Buffer.from(chunk))
  }
  return JSON.parse(Buffer.concat(chunks, size))
}

export function createApi(baseUrl, token) {
  const base = new URL(baseUrl)
  if (base.protocol !== 'http:' || base.username || base.password || base.pathname !== '/'
    || base.search || base.hash) throw new Error('invalid API URL')
  const url = (path) => new URL(`/internal/v1/jobs${path}`, base)
  const headers = { authorization: `Bearer ${token}` }
  const timeout = (signal, duration) => AbortSignal.any([signal, AbortSignal.timeout(duration)])

  async function json(path, signal, { id, body } = {}) {
    const response = await fetch(url(path), {
      headers: {
        ...headers,
        ...(id ? { 'x-message-id': id } : {}),
        ...(body === undefined ? {} : { 'content-type': 'application/json' }),
      },
      method: body === undefined ? 'GET' : 'POST',
      body: body === undefined ? undefined : JSON.stringify(body),
      redirect: 'error',
      signal: timeout(signal, 6_000),
    })
    if (!response.ok) {
      await response.body?.cancel()
      throw new ApiError(response.status)
    }
    return response.status === 204 ? undefined : readJson(response.body)
  }

  return {
    status: (signal, id) => json('', signal, { id }),
    delivery: (job, index, action, signal) => json(`/${job}/delivery`, signal, { body: { index, action } }),
    terminal: (job, reason, signal) => json(`/${job}/terminal`, signal, { body: reason }),
    async upload(source, input, signal) {
      const { id, bytes, digest, name } = input
      if (!Number.isSafeInteger(bytes) || bytes < 1 || bytes > MAX_INPUT) throw new Error('input size')
      const uploadSignal = timeout(signal, 90_000)
      let outgoing
      const response = new Promise((resolve, reject) => {
        outgoing = request(url(''), {
          headers: {
            ...headers,
            'content-length': bytes,
            'content-type': 'application/octet-stream',
            'x-message-id': id,
            'x-content-sha256': digest,
            ...(name ? { 'x-display-name': name } : {}),
          },
          method: 'POST',
          signal: uploadSignal,
        }, resolve)
        outgoing.once('error', reject)
      })
      const finished = pipeline(source, matchLength(bytes), outgoing, { signal: uploadSignal })
        .catch((error) => outgoing.destroy(error))
      try {
        const incoming = await response
        if (![200, 202].includes(incoming.statusCode)) {
          incoming.destroy()
          throw new ApiError(incoming.statusCode)
        }
        return { job: await readJson(incoming), created: incoming.statusCode === 202 }
      } finally {
        source.destroy()
        await finished
      }
    },
    async artifact(job, index, expected, signal) {
      if (!Number.isSafeInteger(expected) || expected < 1 || expected > MAX_CLIP) throw new Error('artifact size')
      const downloadSignal = timeout(signal, 40_000)
      const response = await fetch(url(`/${job}/artifacts/${index}`), {
        headers, redirect: 'error', signal: downloadSignal,
      })
      if (!response.ok || response.headers.get('content-length') !== String(expected)
        || response.headers.get('content-type') !== 'video/mp4') {
        await response.body?.cancel()
        throw new ApiError(response.status)
      }
      const output = matchLength(expected)
      // Install an error listener immediately while the caller sets up the consumer.
      void pipeline(Readable.fromWeb(response.body), output, { signal: downloadSignal }).catch(() => {})
      return output
    },
  }
}
