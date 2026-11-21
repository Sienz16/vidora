import { addAbortListener } from 'node:events'
import { setTimeout as delay } from 'node:timers/promises'
import { jidNormalizedUser } from '@whiskeysockets/baileys'
import { phoneJid } from './bot.mjs'
import { connect, restartRequired } from './whatsapp.mjs'

export function configuredBot(env, cli) {
  return {
    pinned: env.BOT_WHATSAPP_NUMBER ? phoneJid(env.BOT_WHATSAPP_NUMBER) : null,
    required: cli || !env.SIDECAR_CONTROL_TOKEN,
  }
}

export function linkedBot(auth, owner, pinned) {
  const id = auth.state.creds.me?.id
  if (typeof id !== 'string') throw new Error('missing linked identity')
  const jid = jidNormalizedUser(id)
  if (phoneJid(jid.split('@')[0]) !== jid || jid === owner || (pinned && jid !== pinned)) {
    throw new Error('wrong linked identity')
  }
  return jid
}

export function createPairing({
  auth, owner, pinned, stop, log, dial = connect, deadlineMs = 180_000,
}) {
  let connected = false
  let current = null
  let pairing = { state: 'idle', code: null, error: null }
  let notifyPaired
  const paired = new Promise((resolve) => { notifyPaired = resolve })
  let notifyOwner
  const configured = new Promise((resolve) => { notifyOwner = resolve })
  async function wait(promise) {
    if (stop.aborted) return null
    let listener
    const stopped = new Promise((resolve) => { listener = addAbortListener(stop, () => resolve(null)) })
    try {
      return await Promise.race([promise, stopped])
    } finally {
      listener[Symbol.dispose]()
    }
  }
  const status = () => {
    const registered = Boolean(auth.state.creds.registered)
    const jid = registered ? linkedBot(auth, owner, pinned) : pinned
    const seconds = pairing.code && current ? Math.ceil((current.deadline - Date.now()) / 1000) : 0
    return {
      paired: registered,
      connected,
      bot: jid ? `••••${jid.split('@')[0].slice(-4)}` : null,
      owner_configured: Boolean(owner),
      pairing: {
        ...pairing,
        code: seconds > 0 ? pairing.code : null,
        expires_in: seconds > 0 ? seconds : null,
      },
    }
  }
  const transition = (state, error = null) => {
    pairing = { state, code: null, error }
    const event = {
      paired: 'pairing_completed', failed: 'pairing_failed', wrong_account: 'pairing_wrong_account',
      cancelled: 'pairing_cancelled', expired: 'pairing_expired',
    }[state]
    log(event)
  }
  async function attempt(active, jid, onCode) {
    const deadline = AbortSignal.timeout(deadlineMs)
    const signal = AbortSignal.any([stop, active.abort.signal, deadline])
    let staged
    try {
      staged = auth.beginPairing()
      let reason
      let retry = 1_000
      do {
        if (signal.aborted) break
        const result = await dial({
          auth: staged, botJid: jid, stop: signal, log, online() {},
          pairing(code) {
            if (signal.aborted || !/^[A-Z0-9]{8}$/.test(code)) return
            pairing = { state: 'code', code, error: null }
            log('pairing_code_issued')
            onCode?.(code)
          },
        })
        reason = result.reason
        if (reason === restartRequired && !signal.aborted) {
          await delay(retry, undefined, { signal }).catch(() => {})
          retry = Math.min(retry * 2, 30_000)
        }
      } while (reason === restartRequired && !signal.aborted)
      if (signal.aborted) {
        if (deadline.aborted && !active.abort.signal.aborted && !stop.aborted) {
          transition('expired', 'pairing_timed_out')
        } else if (pairing.state !== 'cancelled') transition('cancelled')
      } else if (reason === 'identity_mismatch') {
        transition('wrong_account', 'wrong_account')
      } else if (reason === 'paired') {
        try {
          linkedBot(staged, owner, jid)
        } catch {
          transition('wrong_account', 'wrong_account')
          return pairing.state
        }
        staged.commit()
        transition('paired')
        notifyPaired(true)
      } else transition('failed', 'pairing_request_failed')
    } catch {
      if (pairing.state !== 'cancelled') transition('failed', 'pairing_request_failed')
    } finally {
      try {
        staged?.discard()
      } catch {
        transition('failed', 'pairing_request_failed')
      }
      current = null
    }
    return pairing.state
  }
  return {
    status,
    online: (value) => { connected = value },
    configureOwner(number) {
      if (owner) return 'owner_already_configured'
      let selected
      try {
        selected = phoneJid(number)
      } catch {
        return 'invalid_owner_number'
      }
      const bot = auth.state.creds.registered ? linkedBot(auth, null, pinned) : pinned
      if (selected === bot) return 'owner_is_bot'
      auth.saveOwner(selected)
      owner = selected
      notifyOwner(owner)
      return null
    },
    async waitForOwner() {
      return stop.aborted ? null : owner ?? await wait(configured)
    },
    start(number, onCode) {
      if (!owner) return 'owner_required'
      if (auth.state.creds.registered) return 'already_paired'
      if (current) return 'busy'
      let jid
      try {
        jid = number === undefined ? pinned : phoneJid(number)
      } catch {
        return 'invalid_number'
      }
      if (!jid) return 'invalid_number'
      if (jid === owner) return 'owner_is_bot'
      if (pinned && jid !== pinned) return 'number_mismatch'
      pairing = { state: 'requesting', code: null, error: null }
      log('pairing_started')
      const active = { abort: new AbortController(), deadline: Date.now() + deadlineMs }
      current = active
      // Start on the next microtask so even a synchronous failure has a task to await.
      active.done = Promise.resolve().then(() => attempt(active, jid, onCode))
      return null
    },
    cancel() {
      if (auth.state.creds.registered) return 'already_paired'
      if (current && !current.abort.signal.aborted) {
        current.abort.abort()
        transition('cancelled')
      }
      return null
    },
    async waitForPair() {
      if (auth.state.creds.registered) return true
      return await wait(paired)
    },
    async drain() {
      return await current?.done
    },
  }
}
