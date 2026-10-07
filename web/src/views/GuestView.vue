<script setup>
import { onMounted, reactive, ref, watch } from 'vue'
import { api } from '../api'
import { xhrUpload } from '../upload'
import { auth, toast, absoluteUrl, copyText, initAuth } from '../store'
import { fmtBytes, fmtTime } from '../format'
import Icon from '../components/Icon.vue'
import LinkCard from '../components/LinkCard.vue'
import Countdown from '../components/Countdown.vue'
import Modal from '../components/Modal.vue'

const MAX_FILE = 50 * 1024 * 1024 // 50MB, 前端先挡; 后端 413 的 message 也会透出

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
      task.error = e.message // 429/413 等后端 message 原样透出
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
  if (manage.open) loadList(1)
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

onMounted(async () => {
  // 每次进 /guest 都重新拉 (别处分享到访客后, 回来必须看得到新文件);
  // 等启动时的会话探测落定再决定拉不拉 (管理口要登录)
  if (!auth.ready) await initAuth()
  if (auth.user) loadList(1)
})

// 登录/登出联动: 登出清掉管理数据残影, 登录 (含换账号) 重新拉
watch(
  () => auth.user,
  (u) => {
    if (u) {
      loadList(1)
    } else {
      manage.open = false
      manage.items = []
      manage.total = 0
      manage.page = 1
    }
  },
)
</script>

<template>
  <div class="page guest">
    <div class="guest__head">
      <h1 class="page-title">访客空间</h1>
      <p class="muted guest__sub">丢一个文件, 拿一条 24 小时有效的下载地址。不需要登录。</p>
      <button v-if="auth.user" class="btn guest__manage-toggle" @click="toggleManage">
        {{ manage.open ? '返回上传' : '管理列表' }}
      </button>
    </div>

    <template v-if="!manage.open">
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
        <p class="drop__sub mono">单个文件不超过 50MB · 图片 / pdf / zip / 7z / txt / md · 24 小时后自动过期</p>
        <input ref="fileInput" type="file" multiple hidden @change="onPick" />
      </div>

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
          <span>剩余</span>
          <span>过期时间</span>
          <span>操作</span>
        </div>
        <p v-if="!manage.items.length" class="guest__empty muted">{{ manage.loading ? '加载中…' : '访客空间是空的' }}</p>
        <div v-for="it in manage.items" :key="it.id" class="guest__row">
          <span class="guest__cell-name" :title="it.orig_name">{{ it.orig_name }}</span>
          <span class="mono muted">{{ fmtBytes(it.size) }}</span>
          <span class="mono muted">{{ it.source_ip }}</span>
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
  grid-template-columns: minmax(0, 1fr) 90px 110px 80px 140px max-content;
  gap: 12px;
  padding: 10px 16px;
  border-bottom: 1px solid var(--ink);
  color: var(--muted);
  text-transform: uppercase;
  letter-spacing: .08em;
}

.guest__row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 90px 110px 80px 140px max-content;
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
}
</style>
