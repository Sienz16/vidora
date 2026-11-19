import process from 'node:process'
import { isAbsolute } from 'node:path'
import { setTimeout as delay } from 'node:timers/promises'
import { jidNormalizedUser } from '@whiskeysockets/baileys'
import { createApi } from './api.mjs'
import { openAuth } from './auth.mjs'
import { createBot, phoneJid } from './bot.mjs'
import { connect, invalidSession, restartRequired } from './whatsapp.mjs'

const log = (event) => process.stderr.write(`${event}\n`)
const args = process.argv.slice(2)
const pairing = args.length === 1 && args[0] === '--pair'
const stopping = new AbortController()
process.umask(0o077)
process.once('SIGINT', () => stopping.abort())
process.once('SIGTERM', () => stopping.abort())
process.on('unhandledRejection', () => {
  log('sidecar_async_failure')
  process.exitCode = 1
  stopping.abort()
})

async function run() {
  if ((!pairing && args.length > 0) || (pairing && !process.stdout.isTTY)) {
    throw new Error('invalid invocation')
  }
  const owner = phoneJid(process.env.OWNER_WHATSAPP_NUMBER)
  const botJid = phoneJid(process.env.BOT_WHATSAPP_NUMBER)
  const directory = process.env.BAILEYS_AUTH_DIR
  const token = process.env.INTERNAL_API_TOKEN
  if (owner === botJid || !directory || !isAbsolute(directory)
    || !token || !/^[!-~]{32,}$/.test(token)) throw new Error('invalid configuration')
  const api = createApi(process.env.RUST_API_URL, token)
  const auth = openAuth(directory)
  const bot = createBot({ api, owner, stop: stopping.signal, log })
  try {
    const { creds } = auth.state
    if (creds.me?.id && jidNormalizedUser(creds.me.id) !== botJid) throw new Error('wrong linked account')
    if (!pairing && !creds.registered) {
      log('sidecar_pairing_required')
      return 1
    }
    const stop = pairing
      ? AbortSignal.any([stopping.signal, AbortSignal.timeout(180_000)]) : stopping.signal
    let retry = 1_000
    while (!stop.aborted) {
      const { reason, opened } = await connect({
        auth, botJid, bot, stop, log,
        pairing: pairing ? (code) => process.stdout.write(`Pairing code: ${code}\n`) : undefined,
      })
      await bot.drained()
      if (reason === 'paired') {
        auth.save()
        log('pairing_completed')
        return 0
      }
      if (invalidSession.has(reason) || reason === 'session_write_failed'
        || (pairing && reason !== restartRequired && !stop.aborted)) {
        log('sidecar_session_unavailable')
        return 1
      }
      if (opened) retry = 1_000
      if (!stop.aborted) {
        await delay(retry, undefined, { signal: stop }).catch(() => {})
        retry = Math.min(retry * 2, 30_000)
      }
    }
    if (pairing && !stopping.signal.aborted) log('pairing_timed_out')
    return pairing && !stopping.signal.aborted ? 1 : 0
  } finally {
    auth.close()
  }
}

try {
  process.exitCode = await run() || process.exitCode || 0
} catch {
  log('sidecar_startup_or_shutdown_failed')
  process.exitCode = 1
}
