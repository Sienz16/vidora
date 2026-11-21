import { Readable } from 'node:stream'
import { pipeline } from 'node:stream/promises'

export const uuid = /^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/i
const jobStates = ['receiving', 'processing', 'ready', 'delivering', 'delivered', 'failed', 'expired']
const jobErrors = ['unsupported_hdr', 'input_size', 'processing_failed', 'delivery_failed',
  'delivery_uncertain', 'artifact_missing', 'expired', 'shutdown', 'processing_interrupted']
const pairingStates = ['idle', 'requesting', 'code', 'paired', 'failed', 'expired', 'cancelled', 'wrong_account']
const pairingErrors = ['invalid_number', 'number_mismatch', 'already_paired', 'busy', 'wrong_account',
  'pairing_request_failed', 'pairing_timed_out', 'unauthorized', 'unreachable', 'invalid_request']
const mime = { video: 'video/mp4', photo: 'image/jpeg' }
const byteCap = { video: 15_800_000, photo: 4_900_000 }

function jpegThumbnail(value) {
  if (typeof value !== 'string' || value.length > 65_536 || !/^[A-Za-z0-9+/]+={0,2}$/.test(value)) return null
  const data = Buffer.from(value, 'base64')
  return data.length >= 3 && data[0] === 255 && data[1] === 216 && data[2] === 255
    && data.toString('base64') === value ? value : null
}

export function safeJob(job) {
  if (job === null) return null
  if (!job || typeof job.id !== 'string' || !uuid.test(job.id) || !Object.hasOwn(mime, job.kind)
    || !jobStates.includes(job.state) || !Number.isInteger(job.next_index)
    || job.next_index < 0 || job.next_index > 32 || typeof job.pending !== 'boolean'
    || !Array.isArray(job.sizes) || job.sizes.length > 32
    || job.sizes.some((size) => !Number.isInteger(size) || size < 1 || size > byteCap[job.kind])) {
    throw new Error('invalid upstream job')
  }
  const artifacts = Array.isArray(job.artifacts) ? job.artifacts.slice(0, 32).map((artifact) => {
    const safe = {}
    if (artifact?.mime === mime[job.kind]) safe.mime = artifact.mime
    if (job.kind === 'photo') {
      if (Number.isInteger(artifact?.width) && artifact.width > 0 && artifact.width <= 3840) safe.width = artifact.width
      if (Number.isInteger(artifact?.height) && artifact.height > 0 && artifact.height <= 3840) safe.height = artifact.height
      const thumbnail = jpegThumbnail(artifact?.thumbnail)
      if (thumbnail) safe.thumbnail = thumbnail
    }
    return safe
  }) : []
  return {
    id: job.id, kind: job.kind, state: job.state, next_index: job.next_index,
    pending: job.pending, sizes: job.sizes, artifacts,
    error: job.error == null ? null : (jobErrors.includes(job.error) ? job.error : 'job_failed'),
  }
}

function safeConnection(status) {
  if (!status || typeof status.paired !== 'boolean' || typeof status.connected !== 'boolean'
    || typeof status.owner_configured !== 'boolean' || !pairingStates.includes(status.pairing?.state)) {
    throw new Error('invalid upstream connection')
  }
  const { state, code, expires_in: seconds, error } = status.pairing
  const hasCode = !status.paired && state === 'code' && typeof code === 'string' && /^[A-Z0-9]{8}$/.test(code)
    && Number.isInteger(seconds) && seconds > 0 && seconds <= 180
  return {
    paired: status.paired, connected: status.paired && status.connected,
    bot: typeof status.bot === 'string' && /^••••[0-9]{4}$/.test(status.bot) ? status.bot : null,
    owner_configured: status.owner_configured,
    pairing: {
      state, code: hasCode ? code : null, expires_in: hasCode ? seconds : null,
      error: pairingErrors.includes(error) ? error : null,
    },
  }
}

async function readJson(response, limit) {
  if (!response.body || Number(response.headers.get('content-length')) > limit) {
    await response.body?.cancel()
    throw new Error('upstream body size')
  }
  const reader = response.body.getReader()
  const chunks = []
  let size = 0
  try {
    for (;;) {
      const { done, value } = await reader.read()
      if (done) break
      size += value.length
      if (size > limit) throw new Error('upstream body size')
      chunks.push(value)
    }
    return JSON.parse(Buffer.concat(chunks).toString())
  } finally {
    await reader.cancel()
  }
}

export function createUpstream(config, fetcher = fetch) {
  const get = (service, path, options = {}) => fetcher(service.url + path, {
    ...options,
    redirect: 'error',
    headers: { authorization: `Bearer ${service.token}`, accept: 'application/json', ...options.headers },
    signal: options.signal ?? AbortSignal.timeout(6_000),
  })
  async function json(service, path, limit) {
    const res = await get(service, path)
    if (!res.ok) {
      await res.body?.cancel()
      throw new Error('upstream unavailable')
    }
    return readJson(res, limit)
  }
  return {
    async state() {
      const [connection, job] = await Promise.allSettled([
        json(config.control, '/control/status', 16_384).then(safeConnection),
        json(config.rust, '/internal/v1/jobs', 65_536).then(safeJob),
      ])
      return {
        connection: connection.value ?? null, connection_reachable: connection.status === 'fulfilled',
        job: job.value ?? null, job_error: job.status === 'fulfilled' ? null : 'unavailable',
      }
    },
    async pair(method, body) {
      try {
        const res = await get(config.control, '/control/pair', {
          method,
          ...(method === 'POST' ? { headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) } : {}),
        })
        if ((method === 'POST' && res.status === 202) || (method === 'DELETE' && res.status === 204)) {
          await res.body?.cancel()
          return [res.status, res.status === 202 ? { accepted: true } : undefined]
        }
        if ([400, 401, 409].includes(res.status)) {
          const value = await readJson(res, 16_384)
          if (pairingErrors.includes(value?.error)) return [res.status, { error: value.error }]
        } else await res.body?.cancel()
      } catch {}
      return [503, { error: 'unreachable' }]
    },
    async artifact(res, id, index, send) {
      const abort = new AbortController()
      const close = () => abort.abort()
      res.once('close', close)
      try {
        const path = `/internal/v1/jobs/${id}`
        const job = safeJob(await json(config.rust, path, 65_536))
        if (!job || job.id !== id || !['ready', 'delivering'].includes(job.state)
          || job.pending || index !== job.next_index || index >= job.sizes.length) {
          return send(404, { error: 'not_found' })
        }
        const signal = AbortSignal.any([abort.signal, AbortSignal.timeout(60_000)])
        signal.throwIfAborted()
        const upstream = await get(config.rust, `${path}/artifacts/${index}`, {
          signal, headers: { accept: mime[job.kind] },
        })
        const type = upstream.headers.get('content-type')
        const length = upstream.headers.get('content-length')
        if (upstream.status !== 200 || !upstream.body || type !== mime[job.kind]
          || length !== String(job.sizes[index])) {
          await upstream.body?.cancel()
          return upstream.status === 404 ? send(404, { error: 'not_found' }) : send(502, { error: 'unavailable' })
        }
        const ext = type === 'image/jpeg' ? 'jpg' : 'mp4'
        res.writeHead(200, {
          'Content-Type': type, 'Content-Length': length,
          'Content-Disposition': `attachment; filename="vidora-${id.slice(0, 8)}-${index}.${ext}"`,
        })
        await pipeline(Readable.fromWeb(upstream.body), res, { signal })
      } catch {
        if (!res.headersSent && !res.destroyed) send(502, { error: 'unavailable' })
        else res.destroy()
      } finally {
        res.off('close', close)
      }
    },
  }
}
