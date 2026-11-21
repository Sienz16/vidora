import { createHash, randomBytes, timingSafeEqual } from 'node:crypto'

const digest = (value) => createHash('sha256').update(value).digest()
const random = () => randomBytes(32).toString('hex')

export const equal = (a, b) => typeof a === 'string' && timingSafeEqual(digest(a), digest(b))

export function createSessions({ ttl, secure, now = Date.now }) {
  const sessions = new Map()
  const cookie = (id, maxAge) => `vidora_session=${id}; HttpOnly; SameSite=Strict; Path=/; Max-Age=${maxAge}${secure ? '; Secure' : ''}`
  return {
    create() {
      for (const [id, session] of sessions) if (session.expires <= now()) sessions.delete(id)
      if (sessions.size >= 32) sessions.delete(sessions.keys().next().value)
      const id = random()
      const session = { id, csrf: random(), expires: now() + ttl * 1000 }
      sessions.set(id, session)
      return { session, cookie: cookie(id, ttl) }
    },
    get(header) {
      const ids = (header ?? '').split(';').map((value) => value.trim())
        .filter((value) => value.startsWith('vidora_session='))
      if (ids.length !== 1) return undefined
      const id = ids[0].slice('vidora_session='.length)
      const session = sessions.get(id)
      if (session && session.expires <= now()) {
        sessions.delete(id)
        return undefined
      }
      return session
    },
    remove(session) {
      if (session) sessions.delete(session.id)
      return cookie('', 0)
    },
  }
}

export function rateLimit(cap, duration, now = Date.now) {
  const entries = new Map()
  return {
    retry(key, limit) {
      const time = now()
      for (const [id, entry] of entries) if (entry.expires <= time) entries.delete(id)
      const entry = entries.get(key)
      if (entry?.count >= limit) return Math.ceil((entry.expires - time) / 1000)
      return !entry && entries.size >= cap ? Math.ceil(duration / 1000) : 0
    },
    hit(key) {
      const entry = entries.get(key)
      if (entry) entry.count++
      else entries.set(key, { count: 1, expires: now() + duration })
    },
    clear(key) { entries.delete(key) },
  }
}
