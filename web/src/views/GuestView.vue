<script setup>
import { onMounted, reactive, ref, watch } from 'vue'
import { api } from '../api'
import { xhrUpload } from '../upload'
import { auth, toast, absoluteUrl, copyText, initAuth } from '../store'
import { fmtBytes, fmtTime, normalizeGuestCode, groupGuestCode } from '../format'
import Icon from '../components/Icon.vue'
import LinkCard from '../components/LinkCard.vue'
import Countdown from '../components/Countdown.vue'
import Modal from '../components/Modal.vue'

const MAX_FILE = 50 * 1024 * 1024 // 50MB, 前端先挡; 后端 413 的 message 也会透出
const KEY_STORE = 'yukipan_guest_key' // sessionStorage: 标签页内有效

// ---------- 密钥门 ----------
const gate = reactive({
  checking: true, // 进门先恢复/复核本标签页存过的密钥
  entered: false,
  code: '',
  expiresAt: '',
  input: '',
  error: '',
  busy: false,
})

function loadStoredKey() {
  try {
    return JSON.parse(sessionStorage.getItem(KEY_STORE) || 'null')
  } catch {
    return null
  }
}

function clearKey() {
  sessionStorage.removeItem(KEY_STORE)
  gate.entered = false
  gate.code = ''
  gate.expiresAt = ''
}

function onCodeInput(e) {
  gate.input = groupGuestCode(e.target.value)
  gate.error = ''
}

async function enter() {
  const code = normalizeGuestCode(gate.input)
  if (code.length !== 8) {
    gate.error = '密钥是 8 位短码'
    return
  }
  if (gate.busy) return
  gate.busy = true
  gate.error = ''
  try {
    const data = await api.post('/api/guest/verify', { code })
    sessionStorage.setItem(KEY_STORE, JSON.stringify({ code, expires_at: data.expires_at }))
    gate.entered = true
    gate.code = code
    gate.expiresAt = data.expires_at
    gate.input = ''
  } catch (e) {
    // 403「密钥无效或已过期」/ 429 带剩余时间, 信封 message 原样展示
    gate.error = e.message
  } finally {
    gate.busy = false
  }
}

const dragging = ref(false)
const fileInput = ref(null)
const uploads = reactive([]) // 进行中
const done = reactive([]) // 成功卡片 (链接 + 倒计时)

let seq = 0

function pick() {
  fileInput.value?.click()
}

function onPick(e) {
  start(Array.from(e.target.files || []))
  e.target.value = ''
}

function onDrop(e) {
  dragging.value = false
  start(Array.from(e.dataTransfer?.files || []))
}

async function start(files) {
  for (const file of files) {
    const task = reactive({
      id: ++seq,
      name: file.name,
      size: file.size,
      progress: 0,
      error: '',
      abort: null,
    })
    if (file.size > MAX_FILE) {
      task.error = '超过 50MB, 访客空间放不下'
      uploads.push(task)
      continue
    }
    uploads.push(task)
    try {
      const up = xhrUpload('/api/guest/upload', file, {
        headers: { 'X-Guest-Key': gate.code },
        onProgress: (loaded, total) => {
          task.progress = total ? (loaded / total) * 100 : 0
        },
      })
      task.abort = up.abort
      const data = await up.promise
      done.unshift(data)
      const i = uploads.findIndex((t) => t.id === task.id)
      if (i >= 0) uploads.splice(i, 1)
      // 登录态下管理列表同步刷新 (别让用户切过去看到旧列表)
      if (auth.user) loadList(1)
    } catch (e) {
      // 403 = 密钥中途过期/被吊销: 清掉存密钥, 退回输入页
      if (e.status === 403) {
        task.error = '密钥已失效'
        clearKey()
        gate.error = e.message
        toast('密钥已失效, 请重新输入', 'error')
      } else {
        task.error = e.message // 429/413 等后端 message 原样透出
      }
    }
  }
}

function cancel(task) {
  if (task.abort) task.abort()
  task.error = '已取消'
}

function dismiss(task) {
  const i = uploads.findIndex((t) => t.id === task.id)
  if (i >= 0) uploads.splice(i, 1)
}

// ---------- 管理 (登录后) ----------
const manage = reactive({ open: false, items: [], total: 0, page: 1, loading: false, clearConfirm: false })

async function loadList(p = 1) {
  manage.loading = true
  try {
    const data = await api.get('/api/guest/list', { page: p, per_page: 20 })
    manage.items = data.items
    manage.total = data.total
    manage.page = data.page
  } catch (e) {
    toast(e.message, 'error')
  } finally {
    manage.loading = false
  }
}

function toggleManage() {
  manage.open = !manage.open
  if (manage.open) {
    loadList(1)
    loadKeys()
  }
}

async function doDelete(item) {
  try {
    await api.post('/api/guest/delete', { id: item.id })
    toast('已删除')
    // 当前页删光了就回退一页, 免得停在空页上
    if (manage.items.length === 1 && manage.page > 1) loadList(manage.page - 1)
    else loadList(manage.page)
  } catch (e) {
    toast(e.message, 'error')
  }
}

async function doClear() {
  try {
    const r = await api.post('/api/guest/clear')
    toast(`已清空 ${r.cleared} 条`)
    manage.clearConfirm = false
    loadList(1)
  } catch (e) {
    toast(e.message, 'error')
  }
}

// ---------- 密钥管理 (登录后) ----------
const keys = reactive({
  items: [],
  loading: false,
  ttl: '1h',
  note: '',
  busy: false,
  created: null, // 刚签发的密钥 (大字展示 + 复制)
  revokeTarget: null,
  revoking: false,
})

async function loadKeys() {
  keys.loading = true
  try {
    keys.items = (await api.get('/api/guest/keys')).items
  } catch (e) {
    toast(e.message, 'error')
  } finally {
    keys.loading = false
  }
}

async function issueKey() {
  if (keys.busy) return
  keys.busy = true
  try {
    keys.created = await api.post('/api/guest/keys', {
      ttl: keys.ttl,
      note: keys.note.trim() || undefined,
    })
    keys.note = ''
    toast('密钥已签发')
    loadKeys()
  } catch (e) {
    toast(e.message, 'error')
  } finally {
    keys.busy = false
  }
}

// active | expired | revoked
function keyStatus(k) {
  if (k.revoked_at) return 'revoked'
  return new Date(k.expires_at).getTime() > Date.now() ? 'active' : 'expired'
}

const KEY_STATUS_TEXT = { active: '有效', expired: '已过期', revoked: '已吊销' }

async function doRevoke() {
  keys.revoking = true
  try {
    await api.post('/api/guest/keys/revoke', { id: keys.revokeTarget.id })
    toast('密钥已吊销')
    keys.revokeTarget = null
    loadKeys()
    loadList(1) // 吊销连带删除该密钥全部文件
  } catch (e) {
    toast(e.message, 'error')
  } finally {
    keys.revoking = false
  }
}

onMounted(async () => {
  // 每次进 /guest 都重新拉 (别处分享到访客后, 回来必须看得到新文件);
  // 等启动时的会话探测落定再决定拉不拉 (管理口要登录)
  if (!auth.ready) await initAuth()
  if (auth.user) {
    loadList(1)
    loadKeys()
  }
  // 恢复本标签页验证过的密钥, 复核一次 (可能已被吊销/过期)
  const stored = loadStoredKey()
  if (stored && stored.code) {
    try {
      const data = await api.post('/api/guest/verify', { code: stored.code })
      gate.entered = true
      gate.code = stored.code
      gate.expiresAt = data.expires_at
    } catch {
      clearKey()
    }
  }
  gate.checking = false
})

// 登录/登出联动: 登出清掉管理数据残影, 登录 (含换账号) 重新拉
watch(
  () => auth.user,
  (u) => {
    if (u) {
      loadList(1)
      loadKeys()
    } else {
      manage.open = false
      manage.items = []
      manage.total = 0
      manage.page = 1
      keys.items = []
      keys.created = null
    }
  },
)
</script>

<template>
  <div class="page guest">
    <div class="guest__head">
      <h1 class="page-title">访客空间</h1>
      <p class="muted guest__sub">持临时密钥上传文件, 拿到一条限时下载地址。不需要账号。</p>
      <button v-if="auth.user" class="btn guest__manage-toggle" @click="toggleManage">
        {{ manage.open ? '返回上传' : '管理列表' }}
      </button>
    </div>

    <template v-if="!manage.open">
      <!-- 密钥门 -->
      <div v-if="gate.checking" class="gate card">
        <p class="muted">检查密钥…</p>
      </div>
      <div v-else-if="!gate.entered" class="gate card">
        <Icon name="link" :size="30" class="gate__icon" />
        <p class="gate__main">这扇门需要临时密钥</p>
        <p class="gate__sub muted">向管理员要一枚 8 位短码, 在有效期内可以上传文件。</p>
        <form class="gate__form" @submit.prevent="enter">
          <input
            class="input gate__input"
            :value="gate.input"
            placeholder="XXXX-XXXX"
            maxlength="9"
            autocomplete="off"
            spellcheck="false"
            autofocus
            @input="onCodeInput"
          />
          <button class="btn btn--primary" type="submit" :disabled="gate.busy">
            {{ gate.busy ? '验证中…' : '进入' }}
          </button>
        </form>
        <p v-if="gate.error" class="gate__error">{{ gate.error }}</p>
      </div>

      <template v-else>
        <!-- 密钥状态条 -->
        <div class="keystate card">
          <Icon name="check" :size="13" class="keystate__icon" />
          <span class="mono keystate__code">{{ groupGuestCode(gate.code) }}</span>
          <span class="muted keystate__at">有效期至 {{ fmtTime(gate.expiresAt) }}</span>
          <span class="keystate__remain">剩余 <Countdown :expires-at="gate.expiresAt" /></span>
          <button class="btn btn--ghost btn--small keystate__exit" title="退出当前密钥" @click="clearKey">
            <Icon name="close" :size="12" />
          </button>
        </div>

        <!-- 拖拽区 -->
        <div
          class="drop card"
          :class="{ 'drop--drag': dragging }"
          @dragover.prevent="dragging = true"
          @dragleave.prevent="dragging = false"
          @drop.prevent="onDrop"
          @click="pick"
        >
          <Icon name="upload" :size="34" class="drop__icon" />
          <p class="drop__main">把文件拖到这里, 或点击选择</p>
          <p class="drop__sub mono">单个文件不超过 50MB · 图片 / pdf / zip / 7z / txt / md · 随密钥过期自动失效</p>
          <input ref="fileInput" type="file" multiple hidden @change="onPick" />
        </div>
      </template>

      <!-- 进行中 / 失败 -->
      <div v-if="uploads.length" class="guest__uploads card">
        <div v-for="t in uploads" :key="t.id" class="guest__urow">
          <span class="guest__uname" :title="t.name">{{ t.name }}</span>
          <template v-if="!t.error">
            <span class="mono muted">{{ Math.round(t.progress) }}%</span>
            <span class="rail guest__rail"><span class="rail__fill" :style="{ '--p': t.progress / 100 }" /></span>
            <button class="btn btn--ghost btn--small" @click="cancel(t)">取消</button>
          </template>
          <template v-else>
            <span class="guest__uerr">{{ t.error }}</span>
            <button class="btn btn--ghost btn--small" @click="dismiss(t)"><Icon name="close" :size="12" /></button>
          </template>
        </div>
      </div>

      <!-- 成功卡片 -->
      <div v-if="done.length" class="guest__done">
        <LinkCard v-for="d in done" :key="d.url" :url="d.url" :expires-at="d.expires_at" :name="d.orig_name" />
      </div>
    </template>

    <!-- 管理列表 -->
    <template v-else>
      <!-- 密钥管理 -->
      <section class="keys card">
        <div class="keys__issue">
          <h2 class="keys__title">密钥管理</h2>
          <div class="keys__form">
            <select v-model="keys.ttl" class="input keys__ttl">
              <option value="30m">30 分钟</option>
              <option value="1h">1 小时</option>
              <option value="24h">24 小时</option>
            </select>
            <input v-model="keys.note" class="input keys__note" placeholder="备注 (可空, 给谁用)" maxlength="50" @keyup.enter="issueKey" />
            <button class="btn btn--primary" :disabled="keys.busy" @click="issueKey">
              <Icon name="plus" /> 签发密钥
            </button>
          </div>
        </div>

        <!-- 刚签发的密钥: 大字分组展示 + 一键复制 -->
        <div v-if="keys.created" class="keys__created">
          <span class="keys__bigcode">{{ groupGuestCode(keys.created.code) }}</span>
          <span class="keys__created-meta">
            <span v-if="keys.created.note" class="keys__created-note">{{ keys.created.note }}</span>
            <span class="muted">有效期至 {{ fmtTime(keys.created.expires_at) }} · 剩余 <Countdown :expires-at="keys.created.expires_at" /></span>
          </span>
          <span class="keys__created-ops">
            <button class="btn btn--primary btn--small" @click="copyText(groupGuestCode(keys.created.code))">
              <Icon name="copy" :size="13" /> 复制
            </button>
            <button class="btn btn--ghost btn--small" @click="keys.created = null"><Icon name="close" :size="12" /></button>
          </span>
        </div>

        <div class="keys__thead mono">
          <span>密钥</span>
          <span>备注</span>
          <span>签发时间</span>
          <span>剩余</span>
          <span>文件数</span>
          <span>状态</span>
          <span>操作</span>
        </div>
        <p v-if="!keys.items.length" class="guest__empty muted">{{ keys.loading ? '加载中…' : '还没有签发过密钥' }}</p>
        <div
          v-for="k in keys.items"
          :key="k.id"
          class="keys__row"
          :class="{ 'keys__row--dead': keyStatus(k) !== 'active' }"
        >
          <span class="mono keys__code">{{ groupGuestCode(k.code) }}</span>
          <span class="keys__note-cell" :title="k.note">{{ k.note || '—' }}</span>
          <span class="mono muted">{{ fmtTime(k.created_at) }}</span>
          <Countdown v-if="keyStatus(k) === 'active'" :expires-at="k.expires_at" />
          <span v-else class="mono muted">—</span>
          <span class="mono muted">{{ k.file_count }}</span>
          <span class="keys__status" :class="`keys__status--${keyStatus(k)}`">{{ KEY_STATUS_TEXT[keyStatus(k)] }}</span>
          <span class="guest__cell-ops">
            <button class="btn btn--ghost btn--small" title="复制密钥" @click="copyText(groupGuestCode(k.code))"><Icon name="copy" :size="13" /></button>
            <button
              v-if="keyStatus(k) === 'active'"
              class="btn btn--ghost btn--small btn--danger"
              title="吊销"
              @click="keys.revokeTarget = k"
            >
              <Icon name="trash" :size="13" />
            </button>
          </span>
        </div>
      </section>

      <div class="guest__adminbar">
        <span class="mono muted">共 {{ manage.total }} 条</span>
        <span class="guest__adminbar-right">
          <button class="btn btn--small" @click="loadList(manage.page)"><Icon name="refresh" :size="13" /> 刷新</button>
          <button class="btn btn--small btn--danger" :disabled="!manage.total" @click="manage.clearConfirm = true">
            <Icon name="trash" :size="13" /> 一键清空
          </button>
        </span>
      </div>
      <div class="guest__table card">
        <div class="guest__thead mono">
          <span>文件</span>
          <span>大小</span>
          <span>来源 IP</span>
          <span>密钥</span>
          <span>剩余</span>
          <span>过期时间</span>
          <span>操作</span>
        </div>
        <p v-if="!manage.items.length" class="guest__empty muted">{{ manage.loading ? '加载中…' : '访客空间是空的' }}</p>
        <div v-for="it in manage.items" :key="it.id" class="guest__row">
          <span class="guest__cell-name" :title="it.orig_name">{{ it.orig_name }}</span>
          <span class="mono muted">{{ fmtBytes(it.size) }}</span>
          <span class="mono muted">{{ it.source_ip }}</span>
          <span class="mono" :class="{ muted: !it.key_code }">{{ it.key_code ? groupGuestCode(it.key_code) : '管理员' }}</span>
          <Countdown :expires-at="it.expires_at" />
          <span class="mono muted">{{ fmtTime(it.expires_at) }}</span>
          <span class="guest__cell-ops">
            <button class="btn btn--ghost btn--small" title="复制链接" @click="copyText(absoluteUrl(it.url))"><Icon name="copy" :size="13" /></button>
            <a class="btn btn--ghost btn--small" :href="it.url" title="下载" download><Icon name="download" :size="13" /></a>
            <button class="btn btn--ghost btn--small btn--danger" title="删除" @click="doDelete(it)"><Icon name="trash" :size="13" /></button>
          </span>
        </div>
      </div>
      <div class="guest__pager">
        <button class="btn btn--small" :disabled="manage.page <= 1" @click="loadList(manage.page - 1)">上一页</button>
        <span class="mono muted">{{ manage.page }} / {{ Math.max(1, Math.ceil(manage.total / 20)) }}</span>
        <button class="btn btn--small" :disabled="manage.page * 20 >= manage.total" @click="loadList(manage.page + 1)">下一页</button>
      </div>
    </template>

    <Modal v-if="manage.clearConfirm" title="一键清空" @close="manage.clearConfirm = false">
      <p class="guest__confirm">清空访客空间的全部 {{ manage.total }} 条文件? 链接会立刻失效。</p>
      <template #footer>
        <button class="btn" @click="manage.clearConfirm = false">取消</button>
        <button class="btn btn--primary" @click="doClear">全部清空</button>
      </template>
    </Modal>

    <Modal v-if="keys.revokeTarget" title="吊销密钥" @close="keys.revokeTarget = null">
      <p class="guest__confirm">
        吊销密钥 <strong class="mono">{{ keys.revokeTarget ? groupGuestCode(keys.revokeTarget.code) : '' }}</strong>?
        它会立刻不能上传, 并且<strong class="guest__danger">连带删除该密钥上传的全部 {{ keys.revokeTarget?.file_count ?? 0 }} 个文件</strong>, 链接即刻失效。
      </p>
      <template #footer>
        <button class="btn" @click="keys.revokeTarget = null">取消</button>
        <button class="btn btn--primary" :disabled="keys.revoking" @click="doRevoke">吊销</button>
      </template>
    </Modal>
  </div>
</template>

<style scoped>
.guest__head {
  position: relative;
  margin-bottom: 26px;
}

.guest__sub {
  margin-top: 8px;
  font-size: 14px;
}

.guest__manage-toggle {
  position: absolute;
  top: 0;
  right: 0;
}

.drop {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 12px;
  padding: 64px 20px;
  border-style: dashed;
  border-width: 1.5px;
  cursor: pointer;
  text-align: center;
  transition: border-color .25s var(--ease-out), background .25s var(--ease-out);
}

.drop:hover,
.drop--drag {
  border-color: var(--red);
  background: rgba(228, 60, 74, .04);
}

.drop__icon {
  color: var(--muted);
  transition: color .25s var(--ease-out), transform .3s var(--ease-out);
}

.drop:hover .drop__icon,
.drop--drag .drop__icon {
  color: var(--red);
  transform: translateY(-3px);
}

.drop__main {
  font-size: 16px;
}

.drop__sub {
  color: var(--muted);
}

.guest__uploads {
  margin-top: 16px;
  padding: 6px 16px;
}

.guest__urow {
  display: grid;
  grid-template-columns: minmax(120px, 1fr) auto minmax(100px, 200px) auto;
  align-items: center;
  gap: 12px;
  padding: 8px 0;
  border-bottom: 1px solid var(--softer);
  font-size: 13px;
}

.guest__urow:last-child {
  border-bottom: 0;
}

.guest__uname {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.guest__uerr {
  grid-column: 2 / 4;
  color: var(--red);
  font-size: 12.5px;
}

.guest__done {
  margin-top: 18px;
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.guest__adminbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 12px;
}

.guest__adminbar-right {
  display: flex;
  gap: 8px;
}

.guest__thead {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 90px 110px 100px 80px 140px max-content;
  gap: 12px;
  padding: 10px 16px;
  border-bottom: 1px solid var(--ink);
  color: var(--muted);
  text-transform: uppercase;
  letter-spacing: .08em;
}

.guest__row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 90px 110px 100px 80px 140px max-content;
  gap: 12px;
  align-items: center;
  padding: 8px 16px;
  border-bottom: 1px solid var(--softer);
  font-size: 13.5px;
}

.guest__row:last-child {
  border-bottom: 0;
}

.guest__cell-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.guest__cell-ops {
  display: flex;
  gap: 2px;
  justify-self: end;
}

.guest__empty {
  padding: 40px 16px;
  text-align: center;
}

.guest__pager {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 14px;
  padding: 18px 0;
}

.guest__confirm {
  font-size: 14px;
  line-height: 1.7;
}

.guest__danger {
  color: var(--red);
}

/* ---------- 密钥门 ---------- */
.gate {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 12px;
  padding: 56px 20px;
  text-align: center;
}

.gate__icon {
  color: var(--muted);
}

.gate__main {
  font: 700 1.15rem var(--font-display);
}

.gate__sub {
  font-size: 13px;
  margin-top: -6px;
}

.gate__form {
  display: flex;
  gap: 10px;
  margin-top: 8px;
  width: min(320px, 100%);
}

.gate__input {
  flex: 1;
  font: 16px var(--font-mono);
  letter-spacing: .12em;
  text-align: center;
  text-transform: uppercase;
}

.gate__error {
  font-size: 13px;
  color: var(--red);
}

/* ---------- 密钥状态条 ---------- */
.keystate {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 9px 14px;
  margin-bottom: 14px;
  border-left: 2px solid var(--ink);
  font-size: 13px;
}

.keystate__icon {
  color: var(--red);
  flex: none;
}

.keystate__code {
  letter-spacing: .1em;
}

.keystate__at {
  font-size: 12px;
}

.keystate__remain {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: 12px;
  color: var(--muted);
  margin-left: auto;
}

.keystate__exit {
  flex: none;
}

/* ---------- 密钥管理 ---------- */
.keys {
  margin-bottom: 18px;
}

.keys__issue {
  display: flex;
  align-items: center;
  gap: 16px;
  flex-wrap: wrap;
  padding: 14px 16px;
  border-bottom: 1px solid var(--ink);
}

.keys__title {
  font: 700 1.05rem var(--font-display);
}

.keys__form {
  display: flex;
  gap: 10px;
  align-items: center;
  flex: 1;
  flex-wrap: wrap;
  justify-content: flex-end;
}

.keys__ttl {
  width: 110px;
  flex: none;
}

.keys__note {
  width: 200px;
}

.keys__created {
  display: flex;
  align-items: center;
  gap: 16px;
  flex-wrap: wrap;
  padding: 14px 16px;
  border-bottom: 1px solid var(--softer);
  border-left: 2px solid var(--red);
  background: rgba(228, 60, 74, .04);
}

.keys__bigcode {
  font: 700 1.5rem var(--font-mono);
  letter-spacing: .14em;
}

.keys__created-meta {
  display: flex;
  flex-direction: column;
  gap: 3px;
  font-size: 12.5px;
  flex: 1;
  min-width: 180px;
}

.keys__created-note {
  color: var(--ink);
}

.keys__created-ops {
  display: flex;
  gap: 8px;
  align-items: center;
}

.keys__thead {
  display: grid;
  grid-template-columns: 120px minmax(0, 1fr) 130px 90px 60px 70px max-content;
  gap: 12px;
  padding: 10px 16px;
  border-bottom: 1px solid var(--ink);
  color: var(--muted);
  text-transform: uppercase;
  letter-spacing: .08em;
}

.keys__row {
  display: grid;
  grid-template-columns: 120px minmax(0, 1fr) 130px 90px 60px 70px max-content;
  gap: 12px;
  align-items: center;
  padding: 8px 16px;
  border-bottom: 1px solid var(--softer);
  font-size: 13.5px;
}

.keys__row:last-child {
  border-bottom: 0;
}

.keys__row--dead {
  opacity: .45;
}

.keys__code {
  letter-spacing: .08em;
}

.keys__note-cell {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--muted);
}

.keys__status {
  font-size: 12px;
}

.keys__status--active {
  color: var(--ink);
}

.keys__status--expired,
.keys__status--revoked {
  color: var(--muted);
}

.keys__status--revoked {
  text-decoration: line-through;
}

@media (max-width: 720px) {
  .guest__thead {
    display: none;
  }

  .guest__row {
    grid-template-columns: 1fr auto;
  }

  .guest__row .mono {
    display: none;
  }

  .keys__thead {
    display: none;
  }

  .keys__row {
    grid-template-columns: 1fr auto auto;
  }

  .keys__note-cell,
  .keys__row > .mono.muted {
    display: none;
  }

  .keys__form {
    justify-content: stretch;
  }

  .keys__note {
    width: auto;
    flex: 1;
  }
}
</style>
