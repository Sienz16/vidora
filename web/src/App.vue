<script setup>
import { computed, nextTick, onMounted, onUnmounted, ref } from 'vue'

const pairErrors = {
  invalid_number: 'Enter a valid dedicated secondary number.',
  invalid_owner_number: 'Enter the owner number in international format.',
  owner_is_bot: 'Use a different number for the owner and the bot.',
  owner_required: 'Set the owner number before pairing.',
  owner_already_configured: 'The owner number is already configured.',
  number_mismatch: 'Use the configured bot number.',
  already_paired: 'The bot is already paired.',
  busy: 'A pairing attempt is already in progress.',
  wrong_account: 'The linked account did not match. Try the dedicated bot number again.',
  pairing_request_failed: 'Could not request a code. Try again.',
  pairing_timed_out: 'The pairing code expired. Request a new one.',
  unauthorized: 'The sidecar rejected the request.',
  unreachable: 'The sidecar is unreachable.',
  rate_limited: 'Too many pairing requests. Try again later.',
}
const jobErrors = {
  unsupported_hdr: 'HDR source is not supported.',
  input_size: 'Input exceeds the size limit.',
  processing_failed: 'Processing failed.',
  delivery_failed: 'WhatsApp delivery failed.',
  delivery_uncertain: 'Delivery is uncertain. Check WhatsApp before resending.',
  artifact_missing: 'The output file was missing.',
  expired: 'The output expired before delivery.',
  shutdown: 'The service restarted during processing.',
  processing_interrupted: 'The service restarted during processing.',
}
const csrf = ref('')
const tokenInput = ref('')
const number = ref('')
const ownerNumber = ref('')
const loginError = ref('')
const actionError = ref('')
const action = ref(false)
const loading = ref(true)
const state = ref(null)
const clock = ref(Date.now())
const deadline = ref(0)
const heading = ref(null)
let timer
let ticker
let poll
let mounted = false

const connection = computed(() => state.value?.connection)
const job = computed(() => state.value?.job)
const pairing = computed(() => connection.value?.pairing)
const remaining = computed(() => Math.max(0, Math.ceil((deadline.value - clock.value) / 1000)))
const code = computed(() => remaining.value > 0 ? pairing.value?.code : null)
const counting = computed(() => ['requesting', 'code'].includes(pairing.value?.state))
const status = computed(() => {
  if (!state.value) return 'Checking connection'
  if (!state.value.connection_reachable) return 'Sidecar unreachable'
  if (!connection.value?.paired) return 'Not paired'
  return connection.value.connected ? 'Connected' : 'Disconnected'
})
const download = computed(() => {
  const value = job.value
  if (!value || !['ready', 'delivering'].includes(value.state) || value.pending
    || value.next_index >= value.sizes.length) return null
  return `/api/jobs/${value.id}/artifacts/${value.next_index}`
})
const preview = computed(() => {
  if (job.value?.kind !== 'photo') return null
  const available = job.value.artifacts
  const image = available[job.value.next_index]?.thumbnail ?? available.find((item) => item.thumbnail)?.thumbnail
  if (typeof image !== 'string' || !/^\/9j[A-Za-z0-9+/]*={0,2}$/.test(image)) return null
  return `data:image/jpeg;base64,${image}`
})

async function request(path, { method = 'GET', payload, signal } = {}) {
  const headers = {}
  if (payload !== undefined) headers['Content-Type'] = 'application/json'
  if (method !== 'GET' && csrf.value) headers['X-CSRF-Token'] = csrf.value
  const response = await fetch(path, {
    method, signal, headers, cache: 'no-store',
    body: payload === undefined ? undefined : JSON.stringify(payload),
  })
  return { status: response.status, data: response.status === 204 ? null : await response.json() }
}

function reset() {
  clearTimeout(timer)
  poll?.abort()
  csrf.value = ''
  state.value = null
  deadline.value = 0
  number.value = ''
  ownerNumber.value = ''
  actionError.value = ''
}
function refresh() {
  clearTimeout(timer)
  if (mounted && csrf.value && !document.hidden && !poll) void readState()
}
async function readState() {
  const controller = new AbortController()
  poll = controller
  try {
    const result = await request('/api/state', { signal: controller.signal })
    if (controller.signal.aborted) return
    if (result.status === 401) return reset()
    if (result.status !== 200) throw new Error('state unavailable')
    state.value = result.data
    clock.value = Date.now()
    deadline.value = clock.value + (pairing.value?.expires_in ?? 0) * 1000
  } catch {
    if (!controller.signal.aborted) {
      state.value = { connection_reachable: false, job_error: 'unavailable' }
      deadline.value = 0
    }
  } finally {
    poll = undefined
    if (mounted && csrf.value && !document.hidden) timer = setTimeout(refresh, counting.value ? 1000 : 2000)
  }
}
async function signIn() {
  action.value = true
  loginError.value = ''
  const entered = tokenInput.value
  tokenInput.value = ''
  try {
    const result = await request('/api/login', { method: 'POST', payload: { token: entered } })
    if (result.status === 200) {
      csrf.value = result.data.csrf
      refresh()
      await nextTick()
      heading.value?.focus()
    } else loginError.value = result.status === 429 ? 'Too many attempts. Try again in a few minutes.' : 'The admin token was rejected.'
  } catch {
    loginError.value = 'Could not reach the admin service.'
  } finally {
    action.value = false
  }
}
async function mutate(path, method, payload) {
  action.value = true
  actionError.value = ''
  try {
    const result = await request(path, { method, payload })
    if (path === '/api/logout' && [204, 401].includes(result.status)) {
      reset()
      return
    }
    if (result.status === 202) number.value = ''
    if (result.status === 204) {
      deadline.value = 0
      if (path === '/api/owner') ownerNumber.value = ''
    }
    if (result.status >= 400) {
      if (result.status === 401 && result.data?.error === 'unauthorized') {
        const session = await request('/api/session')
        if (session.status === 401) return reset()
      }
      actionError.value = pairErrors[result.data?.error] ?? 'The request could not be completed.'
    }
  } catch {
    actionError.value = 'Could not reach the admin service.'
  } finally {
    action.value = false
    refresh()
  }
}
function pair() {
  return mutate('/api/pair', 'POST', connection.value?.bot ? {} : { number: number.value })
}
function visibility() {
  if (document.hidden) {
    clearTimeout(timer)
    poll?.abort()
  } else refresh()
}
onMounted(async () => {
  mounted = true
  document.addEventListener('visibilitychange', visibility)
  ticker = setInterval(() => { if (!document.hidden) clock.value = Date.now() }, 250)
  try {
    const result = await request('/api/session')
    if (!mounted) return
    if (result.status === 200) {
      csrf.value = result.data.csrf
      refresh()
    }
  } catch {
    loginError.value = 'Could not reach the admin service.'
  } finally {
    loading.value = false
  }
})
onUnmounted(() => {
  mounted = false
  document.removeEventListener('visibilitychange', visibility)
  clearTimeout(timer)
  clearInterval(ticker)
  poll?.abort()
})
</script>

<template>
  <main class="shell">
    <header class="masthead flex flex-wrap items-center justify-between gap-4">
      <div class="flex items-center gap-3">
        <span class="mark" aria-hidden="true"><i></i><i></i><i></i></span>
        <h1 ref="heading" tabindex="-1">Vidora</h1>
      </div>
      <div v-if="csrf" class="flex flex-wrap items-center gap-3">
        <span class="pill" :class="{ connected: connection?.connected }" aria-live="polite">{{ status }}</span>
        <span v-if="connection?.bot" class="masked">{{ connection.bot }}</span>
        <button class="quiet" type="button" :disabled="action" @click="mutate('/api/logout', 'POST')">Lock</button>
      </div>
    </header>

    <p v-if="loading" role="status">Checking session…</p>
    <section v-else-if="!csrf" class="panel login" aria-labelledby="unlock">
      <h2 id="unlock">Unlock administration</h2>
      <form class="grid gap-4" @submit.prevent="signIn">
        <label for="token">Admin token</label>
        <input id="token" v-model="tokenInput" type="password" autocomplete="off" required maxlength="512">
        <button class="primary" type="submit" :disabled="action">Unlock</button>
        <p v-if="loginError" class="error" role="alert">{{ loginError }}</p>
      </form>
    </section>

    <div v-else class="grid gap-5">
      <p v-if="actionError" class="error" role="alert">{{ actionError }}</p>
      <section v-if="connection && !connection.owner_configured" class="panel" aria-labelledby="owner">
        <h2 id="owner">Set the owner number</h2>
        <p>This is the WhatsApp account that can send jobs to Vidora. If you already use this bot, enter your existing owner number.</p>
        <form class="grid gap-3" @submit.prevent="mutate('/api/owner', 'POST', { number: ownerNumber })">
          <label for="owner-number">Owner WhatsApp number in international format</label>
          <input id="owner-number" v-model.trim="ownerNumber" type="tel" autocomplete="off" maxlength="16" required>
          <button class="primary" type="submit" :disabled="action">Save owner number</button>
        </form>
      </section>
      <section v-else-if="connection && !connection.paired" class="panel" aria-labelledby="pair">
        <h2 id="pair">Link the bot phone</h2>
        <p>Use a dedicated secondary number. On that phone, open WhatsApp, go to Linked devices, choose Link a device, then Link with phone number instead.</p>
        <form v-if="!counting" class="grid gap-3" @submit.prevent="pair">
          <p v-if="connection.bot">Using configured bot number <span class="masked">{{ connection.bot }}</span></p>
          <template v-else>
            <label for="number">Bot phone number in international format</label>
            <input id="number" v-model.trim="number" type="tel" autocomplete="off" maxlength="16"
              placeholder="+ country code and number" required>
          </template>
          <button class="primary" type="submit" :disabled="action">{{ pairing?.state === 'idle' ? 'Get pairing code' : 'Get a new code' }}</button>
        </form>
        <div v-else class="grid gap-3">
          <div class="code-box" aria-live="polite">
            <strong v-if="code" class="code">{{ code }}</strong>
            <span v-else>{{ pairing?.state === 'requesting' ? 'Requesting pairing code…' : 'Waiting for pairing status…' }}</span>
          </div>
          <p v-if="code" class="muted">Expires in {{ remaining }} seconds</p>
          <button class="quiet justify-self-start" type="button" :disabled="action" @click="mutate('/api/pair', 'DELETE')">Cancel pairing</button>
        </div>
        <p v-if="pairing?.error" class="error" role="alert">{{ pairErrors[pairing.error] ?? 'Pairing failed.' }}</p>
      </section>

      <section class="panel" aria-labelledby="job">
        <div class="flex flex-wrap items-center justify-between gap-3">
          <h2 id="job">Current job</h2>
          <span v-if="job" class="pill capitalize">{{ job.state }}</span>
        </div>
        <p v-if="state?.job_error">Job status is temporarily unavailable.</p>
        <p v-else-if="!job">Send a video as a document or a photo to the bot from the owner account.</p>
        <div v-else class="grid gap-3">
          <p class="capitalize">{{ job.kind }}</p>
          <div v-if="job.sizes.length" class="grid gap-2">
            <label for="delivery-progress">Delivered {{ Math.min(job.next_index, job.sizes.length) }} of {{ job.sizes.length }}</label>
            <progress id="delivery-progress" :value="job.next_index" :max="job.sizes.length"></progress>
          </div>
          <p v-if="job.error" class="error">{{ jobErrors[job.error] ?? 'Job failed.' }}</p>
          <img v-if="preview" :src="preview" alt="Preview of the prepared photo" width="96">
          <a v-if="download" class="primary justify-self-start" :href="download">
            {{ job.kind === 'photo' ? 'Download photo' : `Download part ${job.next_index + 1}` }}
          </a>
        </div>
        <p class="muted">Downloads are available only while the job is active. Files are deleted after delivery.</p>
      </section>

      <section class="panel" aria-labelledby="guide">
        <h2 id="guide">How to use Vidora</h2>
        <ul class="guide">
          <li>The owner can run one job at a time. Send <code>/help</code> for input limits or <code>/status</code> for progress.</li>
          <li>Forward prepared photos to Status. For videos, save the returned parts and post them manually from your gallery.</li>
          <li>WhatsApp may recompress your media. Results depend on the app version, device, and posting path.</li>
        </ul>
      </section>
    </div>
  </main>
</template>
