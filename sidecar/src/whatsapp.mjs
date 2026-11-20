import { addAbortListener } from 'node:events'
import {
  Browsers, DisconnectReason, generateMessageIDV2, jidNormalizedUser,
  makeWASocket, prepareWAMessageMedia,
} from '@whiskeysockets/baileys'
import { downloadMedia } from './media.mjs'

const silent = {
  level: 'silent',
  child: () => silent,
  trace() {}, debug() {}, info() {}, warn() {}, error() {}, fatal() {},
}

export const invalidSession = new Set([
  DisconnectReason.loggedOut, DisconnectReason.badSession, DisconnectReason.forbidden,
  DisconnectReason.connectionReplaced, DisconnectReason.multideviceMismatch, 'identity_mismatch',
])
export const restartRequired = DisconnectReason.restartRequired

async function bounded(operation, signal, ms, onAbort = () => {}) {
  const deadline = AbortSignal.any([signal, AbortSignal.timeout(ms)])
  deadline.throwIfAborted()
  let listener
  try {
    const aborted = new Promise((_, reject) => {
      listener = addAbortListener(deadline, () => {
        onAbort()
        reject(deadline.reason)
      })
    })
    return await Promise.race([operation(), aborted])
  } finally {
    listener?.[Symbol.dispose]()
  }
}

export async function connect({ auth, botJid, bot, stop, pairing, log, online }) {
  const socket = makeWASocket({
    auth: auth.state,
    browser: Browsers.ubuntu('Chrome'),
    logger: silent,
    markOnlineOnConnect: false,
    syncFullHistory: false,
    shouldSyncHistoryMessage: () => false,
    shouldIgnoreJid: (jid) => /@(?:g\.us|broadcast|newsletter)$/.test(jid),
    getMessage: async () => undefined,
    enableRecentMessageCache: false,
    enableAutoSessionRecreation: false,
    emitOwnEvents: false,
    connectTimeoutMs: 15_000,
    defaultQueryTimeoutMs: 12_000,
  })
  const link = new AbortController()
  const signal = AbortSignal.any([stop, link.signal])
  const end = () => { void socket.end(undefined) }
  const wa = {
    signal,
    lookupLid: (jid) => socket.signalRepository.lidMapping.getPNForLID(jid),
    download: (message, bytes, mediaType = 'document') => downloadMedia(
      message, bytes, mediaType, signal, (original) => socket.updateMediaMessage(original),
    ),
    reply: (jid, text) => bounded(() => socket.sendMessage(jid, { text }), signal, 15_000, end),
    prepare: (stream, index, total, options = {}) => bounded(() => {
      const media = options.kind === 'photo'
        ? {
          image: { stream },
          mimetype: 'image/jpeg',
          caption: total > 1 ? `HD Photo ${index + 1}/${total} — forward to Status` : 'HD Photo — forward to Status',
          ...(Number.isInteger(options.meta?.width) ? { width: options.meta.width } : {}),
          ...(Number.isInteger(options.meta?.height) ? { height: options.meta.height } : {}),
          ...(typeof options.meta?.thumbnail === 'string' && options.meta.thumbnail.length > 0
            ? { jpegThumbnail: options.meta.thumbnail } : {}),
        }
        : {
          document: { stream },
          mimetype: 'video/mp4',
          fileName: `part-${String(index + 1).padStart(2, '0')}.mp4`,
          caption: `Part ${index + 1}/${total}`,
        }
      return prepareWAMessageMedia(media, {
        logger: silent,
        upload: socket.waUploadToServer,
        mediaUploadTimeoutMs: 12_000,
      })
    }, signal, 60_000),
    async send(jid, content) {
      await bounded(async () => {
        const messageId = generateMessageIDV2(socket.user?.id)
        const awaiting = socket.waitForMessage(messageId, 22_000).catch(() => undefined)
        await socket.relayMessage(jid, content, { messageId })
        const ack = await awaiting
        if (ack?.tag !== 'ack' || ack.attrs?.id !== messageId || ack.attrs?.class !== 'message'
          || ack.attrs?.error) throw new Error('delivery unconfirmed')
      }, signal, 24_000, end)
    },
  }
  let interval
  let opened = false
  let requested = false
  let finish
  const complete = new Promise((resolve) => { finish = resolve })
  let accountMismatch = false
  const mismatch = (id) => Boolean(id && jidNormalizedUser(id) !== botJid)
  const credentials = () => {
    if (mismatch(auth.state.creds.me?.id)) {
      accountMismatch = true
      finish('identity_mismatch')
      return
    }
    try {
      auth.save()
    } catch {
      finish('session_write_failed')
    }
  }
  const update = (event) => {
    if (event.connection === 'close') finish(event.lastDisconnect?.error?.output?.statusCode)
    if (event.connection === 'open') {
      if (accountMismatch || !auth.state.creds.registered || !socket.user?.id || mismatch(socket.user.id)) {
        finish('identity_mismatch')
        return
      }
      opened = true
      if (pairing) finish('paired')
      else {
        online(true)
        log('whatsapp_connected')
        void bot.tick(wa)
        interval = setInterval(() => void bot.tick(wa), 2_000)
      }
    }
    if (event.qr && pairing && !requested && !auth.state.creds.registered) {
      requested = true
      const number = botJid.split('@')[0]
      void bounded(() => socket.requestPairingCode(number), signal, 15_000, end)
        .then((code) => pairing(code), () => finish('pairing_request_failed'))
    }
  }
  const upsert = (event) => {
    if (opened && !pairing) void bot.onUpsert(event, wa).catch(() => log('message_handling_failed'))
  }
  socket.ev.on('creds.update', credentials)
  socket.ev.on('connection.update', update)
  socket.ev.on('messages.upsert', upsert)
  const abort = addAbortListener(stop, () => finish('stopped'))
  try {
    return { reason: await complete, opened }
  } finally {
    clearInterval(interval)
    link.abort()
    abort[Symbol.dispose]()
    socket.ev.off('creds.update', credentials)
    socket.ev.off('connection.update', update)
    socket.ev.off('messages.upsert', upsert)
    await socket.end(undefined)
    if (opened && !pairing) {
      online(false)
      log('whatsapp_disconnected')
    }
  }
}
