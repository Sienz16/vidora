import process from 'node:process'
import { once } from 'node:events'
import { rmSync, writeFileSync } from 'node:fs'
import { isAbsolute } from 'node:path'
import { setTimeout as delay } from 'node:timers/promises'
import { createApi } from './api.mjs'
import { openAuth } from './auth.mjs'
import { createBot, phoneJid } from './bot.mjs'
import { controlConfig, createControl } from './control.mjs'
import { configuredBot, createPairing, linkedBot } from './pairing.mjs'
import { connect, invalidSession } from './whatsapp.mjs'

const log = (event) => process.stderr.write(`${event}\n`)
const args = process.argv.slice(2)
const pairing = args.length === 1 && args[0] === '--pair'
const stopping = new AbortController()
process.umask(0o077)
// The pinned Signal dependency also logs session objects outside the Baileys logger.
for (const method of ['log', 'info', 'warn', 'error', 'debug', 'trace', 'dir']) {
  console[method] = () => {}
}
process.once('uncaughtException', () => {
  log('sidecar_uncaught_failure')
  process.exit(1)
})
process.once('SIGINT', () => stopping.abort())
process.once('SIGTERM', () => stopping.abort())
process.on('unhandledRejection', () => {
  log('sidecar_async_failure')
  process.exitCode = 1
  stopping.abort()
})

async function run() {
  if (!pairing && args.length > 0) throw new Error('invalid invocation')
  const { pinned, required } = configuredBot(process.env, pairing)
  if (required && !pinned) {
    log('bot_number_required')
    return 1
  }
  if (pairing && !process.stdout.isTTY) throw new Error('invalid invocation')
  const control = pairing ? null : controlConfig(process.env)
  const envOwner = process.env.OWNER_WHATSAPP_NUMBER ? phoneJid(process.env.OWNER_WHATSAPP_NUMBER) : null
  const directory = process.env.BAILEYS_AUTH_DIR
  const token = process.env.INTERNAL_API_TOKEN
  const marker = process.env.VIDORA_READY_FILE
  if (!directory || !isAbsolute(directory)
    || !token || !/^[!-~]{32,}$/.test(token)
    || (marker && !isAbsolute(marker))) throw new Error('invalid configuration')
  const online = (connected) => {
    if (!marker || pairing) return
    try {
      if (connected) writeFileSync(marker, '')
      else rmSync(marker, { force: true })
    } catch {
      log('sidecar_health_marker_failed')
      process.exitCode = 1
      stopping.abort()
    }
  }
  online(false)
  const api = createApi(process.env.RUST_API_URL, token)
  const auth = openAuth(directory)
  const stop = stopping.signal
  let service
  let server
  try {
    const storedOwner = auth.owner()
    let owner = storedOwner ?? envOwner
    if ((storedOwner && envOwner && storedOwner !== envOwner) || (owner && owner === pinned)) {
      throw new Error('owner configuration mismatch')
    }
    if (!control && !owner) {
      log('owner_number_required')
      return 1
    }
    service = createPairing({ auth, owner, pinned, stop, log })
    const { creds } = auth.state
    if (creds.registered) linkedBot(auth, owner, pinned)
    if (pairing) {
      const error = service.start(undefined, (code) => process.stdout.write(`Pairing code: ${code}\n`))
      if (error) throw new Error('pairing unavailable')
      const outcome = await service.drain()
      return outcome === 'paired' || stop.aborted ? 0 : 1
    }
    if (!control && !creds.registered) {
      log('sidecar_pairing_required')
      return 1
    }
    if (control) {
      server = createControl({ token: control.token, pairing: service })
      server.listen(control.port, control.host)
      await once(server, 'listening')
      owner = await service.waitForOwner()
      if (!owner) return 0
      if (!await service.waitForPair()) return 0
    }
    const botJid = linkedBot(auth, owner, pinned)
    const bot = createBot({ api, owner, stop, log })
    let retry = 1_000
    while (!stop.aborted) {
      const { reason, opened } = await connect({
        auth, botJid, bot, stop, log,
        online(connected) {
          service.online(connected)
          online(connected)
        },
      })
      await bot.drained()
      if (invalidSession.has(reason) || reason === 'session_write_failed') {
        log('sidecar_session_unavailable')
        return 1
      }
      if (opened) retry = 1_000
      if (!stop.aborted) {
        await delay(retry, undefined, { signal: stop }).catch(() => {})
        retry = Math.min(retry * 2, 30_000)
      }
    }
    return 0
  } finally {
    if (server) {
      const closed = new Promise((resolve) => server.close(resolve))
      server.closeAllConnections()
      await closed
    }
    service?.cancel()
    await service?.drain()
    auth.close()
  }
}

try {
  process.exitCode = await run() || process.exitCode || 0
} catch {
  log('sidecar_startup_or_shutdown_failed')
  process.exitCode = 1
}
