import { createHash, timingSafeEqual } from 'node:crypto'
import { createServer } from 'node:http'
import { isIPv4 } from 'node:net'

const hash = (value) => createHash('sha256').update(value).digest()
export const secretPattern = /^[!-~]{32,}$/

export function controlConfig(env) {
  if (env.SIDECAR_CONTROL_TOKEN === undefined) return null
  const token = env.SIDECAR_CONTROL_TOKEN
  const host = env.SIDECAR_CONTROL_BIND ?? '127.0.0.1'
  const value = env.SIDECAR_CONTROL_PORT ?? '8090'
  const port = Number(value)
  if (!secretPattern.test(token) || token === env.INTERNAL_API_TOKEN
    || !isIPv4(host) || host === '0.0.0.0'
    || !/^\d+$/.test(value) || port < 1 || port > 65535) throw new Error('invalid control configuration')
  return { token, host, port }
}

export function createControl({ token, pairing }) {
  const expected = hash(token)
  const server = createServer({
    maxHeaderSize: 4096, requestTimeout: 5_000, headersTimeout: 5_000,
    connectionsCheckingInterval: 1000, keepAliveTimeout: 1000,
  }, async (req, res) => {
    res.setHeader('Cache-Control', 'no-store')
    const send = (status, value) => {
      res.statusCode = status
      if (value === undefined) return res.end()
      res.setHeader('Content-Type', 'application/json')
      res.end(JSON.stringify(value))
    }
    const auth = req.headersDistinct.authorization
    const match = auth?.length === 1 && /^Bearer ([!-~]+)$/.exec(auth[0])
    if (!match || !timingSafeEqual(hash(match[1]), expected)) return send(401, { error: 'unauthorized' })
    const allowed = { '/control/status': ['GET'], '/control/pair': ['POST', 'DELETE'] }[req.url]
    if (!Array.isArray(allowed)) return send(404, { error: 'not_found' })
    if (!allowed.includes(req.method)) {
      res.setHeader('Allow', allowed.join(', '))
      return send(405, { error: 'method_not_allowed' })
    }
    if (req.method !== 'POST' && (req.headers['transfer-encoding'] || Number(req.headers['content-length']) > 0)) {
      res.setHeader('Connection', 'close')
      return send(400, { error: 'invalid_request' })
    }
    if (req.method === 'GET') {
      try {
        return send(200, pairing.status())
      } catch {
        return send(503, { error: 'unavailable' })
      }
    }
    if (req.method === 'DELETE') {
      const error = pairing.cancel()
      return error ? send(409, { error }) : send(204)
    }
    if (req.headersDistinct['content-type']?.length !== 1
      || !/^application\/json(?:\s*;\s*charset=utf-8)?$/i.test(req.headers['content-type'])) {
      return send(400, { error: 'invalid_request' })
    }
    if (Number(req.headers['content-length']) > 1024) {
      res.setHeader('Connection', 'close')
      return send(413, { error: 'invalid_request' })
    }
    let body
    try {
      const chunks = []
      let bytes = 0
      for await (const chunk of req.iterator({ destroyOnReturn: false })) {
        bytes += chunk.length
        if (bytes > 1024) {
          res.setHeader('Connection', 'close')
          return send(413, { error: 'invalid_request' })
        }
        chunks.push(chunk)
      }
      body = JSON.parse(Buffer.concat(chunks).toString())
    } catch {
      return send(400, { error: 'invalid_request' })
    }
    if (!body || typeof body !== 'object' || Array.isArray(body)
      || Object.keys(body).some((key) => key !== 'number')
      || (Object.hasOwn(body, 'number') && typeof body.number !== 'string')) {
      return send(400, { error: 'invalid_request' })
    }
    const error = pairing.start(body.number)
    if (!error) return send(202, { accepted: true })
    return send(['busy', 'already_paired'].includes(error) ? 409 : 400, { error })
  })
  server.maxConnections = 16
  return server
}
