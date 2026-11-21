import { createServer } from 'node:http'
import { isIPv4 } from 'node:net'
import { readFile, realpath } from 'node:fs/promises'
import { extname, resolve, sep } from 'node:path'
import { createSessions, equal, rateLimit } from './session.mjs'
import { createUpstream } from './upstream.mjs'

const csp = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; font-src 'self'; object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'"
const headers = {
  'Content-Security-Policy': csp,
  'X-Content-Type-Options': 'nosniff',
  'Referrer-Policy': 'no-referrer',
  'X-Frame-Options': 'DENY',
  'Permissions-Policy': 'camera=(), microphone=(), geolocation=()',
  'Cross-Origin-Opener-Policy': 'same-origin',
  'Cross-Origin-Resource-Policy': 'same-origin',
  'Cache-Control': 'no-store',
}
const mime = {
  '.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8', '.css': 'text/css; charset=utf-8',
  '.png': 'image/png', '.svg': 'image/svg+xml', '.woff2': 'font/woff2',
}
const asset = /^\/assets\/[A-Za-z0-9_-]+\.[A-Za-z0-9]+$/
const jobArtifact = /^\/api\/jobs\/([a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12})\/artifacts\/(0|[1-9]|[12][0-9]|3[01])$/i
const privateNumber = /^\+?[1-9][0-9]{6,14}$/

export function readConfig(env) {
  const fail = () => { throw new Error('invalid web configuration') }
  const integer = (value, fallback, max) => {
    const raw = value ?? String(fallback)
    if (!/^\d+$/.test(raw) || Number(raw) < 1 || Number(raw) > max) fail()
    return Number(raw)
  }
  const token = (value) => {
    if (typeof value !== 'string' || !/^[!-~]{32,}$/.test(value)) fail()
    return value
  }
  const origin = (value) => {
    const parsed = new URL(value)
    if (!['http:', 'https:'].includes(parsed.protocol) || parsed.origin !== value) fail()
    return parsed
  }
  const publicOrigin = origin(env.WEB_PUBLIC_ORIGIN)
  const hosts = (env.WEB_ALLOWED_HOSTS ?? `${publicOrigin.host},127.0.0.1:3000,localhost:3000`)
    .split(',').map((host) => host.trim().toLowerCase())
  if (!hosts.includes(publicOrigin.host) || hosts.some((host) => !/^[a-z0-9.-]+(?::\d+)?$/.test(host))) fail()
  const config = {
    port: integer(env.PORT, 8080, 65535), bind: env.WEB_BIND_IP ?? '0.0.0.0',
    ttl: integer(env.WEB_SESSION_TTL_SECONDS, 43_200, 2_147_483_647),
    secure: env.WEB_COOKIE_SECURE === '1',
    origin: publicOrigin.origin, hosts: new Set(hosts), admin: token(env.WEB_ADMIN_TOKEN),
    rust: { url: origin(env.RUST_API_URL).origin, token: token(env.INTERNAL_API_TOKEN) },
    control: { url: origin(env.SIDECAR_CONTROL_URL).origin, token: token(env.SIDECAR_CONTROL_TOKEN) },
  }
  if (!isIPv4(config.bind) || ![undefined, '0', '1'].includes(env.WEB_COOKIE_SECURE)
    || new Set([config.admin, config.rust.token, config.control.token]).size !== 3) fail()
  return config
}

async function body(req, res) {
  const contentType = req.headersDistinct['content-type']
  if (contentType?.length !== 1 || !/^application\/json(?:\s*;\s*charset=utf-8)?$/i.test(contentType[0])) return null
  const chunks = []
  let size = 0
  for await (const chunk of req.iterator({ destroyOnReturn: false })) {
    size += chunk.length
    if (size > 1024) {
      res.setHeader('Connection', 'close')
      return null
    }
    chunks.push(chunk)
  }
  try {
    const value = JSON.parse(Buffer.concat(chunks).toString())
    return value && typeof value === 'object' && !Array.isArray(value) ? value : null
  } catch {
    return null
  }
}

export async function createWeb({ config, dist, fetcher, now = Date.now }) {
  const root = await realpath(dist)
  const sessions = createSessions({ ttl: config.ttl, secure: config.secure, now })
  const loginFailures = rateLimit(1024, 300_000, now)
  const pairingStarts = rateLimit(256, 600_000, now)
  const csrfFailures = rateLimit(256, 60_000, now)
  const upstream = createUpstream(config, fetcher)
  const sameOrigin = (req) => req.headersDistinct.origin?.length === 1 && req.headers.origin === config.origin
    && req.headersDistinct['sec-fetch-site']?.length === 1 && req.headers['sec-fetch-site'] === 'same-origin'

  return createServer({
    maxHeaderSize: 8192, requestTimeout: 5_000, headersTimeout: 5_000,
    connectionsCheckingInterval: 1000, keepAliveTimeout: 2000,
  }, async (req, res) => {
    for (const [name, value] of Object.entries(headers)) res.setHeader(name, value)
    const send = (status, data) => {
      if (res.destroyed || res.headersSent) return
      res.statusCode = status
      if (data === undefined) return res.end()
      res.setHeader('Content-Type', 'application/json; charset=utf-8')
      res.end(JSON.stringify(data))
    }
    const missing = () => send(404, { error: 'not_found' })
    const unauthorized = () => send(401, { error: 'unauthorized' })
    const denied = () => send(403, { error: 'forbidden' })
    const limited = (seconds) => {
      res.setHeader('Retry-After', String(seconds))
      send(429, { error: 'rate_limited' })
    }

    try {
      const host = req.headersDistinct.host
      if (host?.length !== 1 || !config.hosts.has(host[0].toLowerCase())) {
        return send(421, { error: 'invalid_host' })
      }
      const path = req.url
      const method = req.method
      if (method === 'GET' && path === '/healthz') return send(204)
      if (method === 'GET' && (path === '/' || asset.test(path))) {
        const name = path === '/' ? '/index.html' : path
        const type = mime[extname(name)]
        if (!type) return missing()
        let content
        try {
          const file = await realpath(resolve(root, '.' + name))
          if (!file.startsWith(root + sep)) return missing()
          content = await readFile(file)
        } catch {
          return missing()
        }
        res.setHeader('Content-Type', type)
        if (path !== '/' && /-[A-Za-z0-9_-]{8,}\./.test(name)) {
          res.setHeader('Cache-Control', 'public, max-age=31536000, immutable')
        }
        return res.end(content)
      }
      if (!path.startsWith('/api/')) return missing()
      if (path === '/api/login' && method === 'POST') {
        if (!sameOrigin(req)) return denied()
        const data = await body(req, res)
        const ip = req.socket.remoteAddress
        const retry = loginFailures.retry(ip, 5)
        if (retry) return limited(retry)
        if (!data || Object.keys(data).length !== 1 || !equal(data.token, config.admin)) {
          loginFailures.hit(ip)
          return unauthorized()
        }
        loginFailures.clear(ip)
        sessions.remove(sessions.get(req.headers.cookie))
        const { session, cookie } = sessions.create()
        res.setHeader('Set-Cookie', cookie)
        return send(200, { csrf: session.csrf })
      }
      const session = sessions.get(req.headers.cookie)
      if (!session) return unauthorized()

      const pair = path === '/api/pair' && ['POST', 'DELETE'].includes(method)
      const logout = path === '/api/logout' && method === 'POST'
      if (pair || logout) {
        const retry = csrfFailures.retry(session.id, 20)
        if (retry) return limited(retry)
        if (!sameOrigin(req) || req.headersDistinct['x-csrf-token']?.length !== 1
          || !equal(req.headers['x-csrf-token'], session.csrf)) {
          csrfFailures.hit(session.id)
          return denied()
        }
        if (logout) {
          res.setHeader('Set-Cookie', sessions.remove(session))
          pairingStarts.clear(session.id)
          csrfFailures.clear(session.id)
          return send(204)
        }
        if (method === 'DELETE') return send(...await upstream.pair(method))
        const data = await body(req, res)
        if (!data || Object.keys(data).some((key) => key !== 'number')) return send(400, { error: 'invalid_request' })
        if (Object.hasOwn(data, 'number') && (typeof data.number !== 'string' || !privateNumber.test(data.number))) {
          return send(400, { error: 'invalid_number' })
        }
        const cooldown = pairingStarts.retry(session.id, 5)
        if (cooldown) return limited(cooldown)
        pairingStarts.hit(session.id)
        return send(...await upstream.pair(method, data))
      }
      if (method !== 'GET') return missing()
      if (path === '/api/session') return send(200, { authenticated: true, csrf: session.csrf })
      if (path === '/api/state') return send(200, await upstream.state())
      const match = jobArtifact.exec(path)
      if (match) return await upstream.artifact(res, match[1].toLowerCase(), Number(match[2]), send)
      return missing()
    } catch {
      return send(500, { error: 'unavailable' })
    }
  })
}
