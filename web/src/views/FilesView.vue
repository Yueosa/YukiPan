<script setup>
import { computed, onMounted, reactive, ref } from 'vue'
import { api, fsFileUrl } from '../api'
import { xhrUpload } from '../upload'
import { hashFile } from '../hash'
import { toast, absoluteUrl, copyText } from '../store'
import { fmtBytes, fmtTime, isImageName, isTextName, joinPath } from '../format'
import Icon from '../components/Icon.vue'
import Modal from '../components/Modal.vue'
import LinkCard from '../components/LinkCard.vue'

// ---------- 浏览状态 ----------
const currentPath = ref('') // 相对根, 无首尾斜杠, '' 为根
const entries = ref([])
const loading = ref(false)
const error = ref('')
const quota = ref(null)
const dragging = ref(false)

const crumbs = computed(() => {
  const segs = currentPath.value ? currentPath.value.split('/') : []
  return segs.map((name, i) => ({ name, path: segs.slice(0, i + 1).join('/') }))
})

async function refresh() {
  loading.value = true
  error.value = ''
  try {
    entries.value = await api.get('/api/fs/list', { path: currentPath.value })
  } catch (e) {
    error.value = e.message
  } finally {
    loading.value = false
  }
}

async function refreshQuota() {
  try {
    quota.value = await api.get('/api/quota')
  } catch {
    quota.value = null
  }
}

function go(path) {
  currentPath.value = path
  refresh()
}

onMounted(() => {
  refresh()
  refreshQuota()
})

// ---------- 上传 ----------
const instantTry = ref(true)
const uploads = reactive([])
const fileInput = ref(null)

function pickFiles() {
  fileInput.value?.click()
}

function onPick(e) {
  startUploads(Array.from(e.target.files || []))
  e.target.value = ''
}

function onDrop(e) {
  dragging.value = false
  const files = Array.from(e.dataTransfer?.files || [])
  if (files.length) startUploads(files)
}

let uploadSeq = 0

async function startUploads(files) {
  // 串行上传: 进度清晰, 也不给后端施压
  for (const file of files) {
    const task = reactive({
      id: ++uploadSeq,
      name: file.name,
      size: file.size,
      phase: 'hash', // hash | upload | done | error
      progress: 0,
      error: '',
      cancelled: false,
      abort: null,
    })
    uploads.push(task)
    try {
      let sha256 = ''
      if (instantTry.value && file.size > 0) {
        task.phase = 'hash'
        sha256 = await hashFile(file, {
          onProgress: (done, total) => {
            task.progress = total ? (done / total) * 100 : 100
          },
          shouldCancel: () => task.cancelled,
        })
        try {
          await api.post('/api/fs/instant', {
            path: currentPath.value,
            name: file.name,
            sha256,
            size: file.size,
          })
          task.phase = 'done'
          task.progress = 100
          continue
        } catch (e) {
          // 404 = 库里没有或不可秒传, 走完整上传; 其余错误 (409 已存在/413 配额) 直接报
          if (e.status !== 404) throw e
        }
      }
      task.phase = 'upload'
      task.progress = 0
      const q = new URLSearchParams({ path: currentPath.value })
      if (sha256) q.set('sha256', sha256)
      const up = xhrUpload(`/api/fs/upload?${q.toString()}`, file, {
        onProgress: (loaded, total) => {
          task.progress = total ? (loaded / total) * 100 : 0
        },
      })
      task.abort = up.abort
      await up.promise
      task.phase = 'done'
      task.progress = 100
    } catch (e) {
      task.phase = 'error'
      task.error = task.cancelled ? '已取消' : e.message
    }
  }
  refresh()
  refreshQuota()
}

function cancelUpload(task) {
  task.cancelled = true
  if (task.abort) task.abort()
  if (task.phase !== 'done') {
    task.phase = 'error'
    task.error = '已取消'
  }
}

function dismissUpload(task) {
  const i = uploads.findIndex((t) => t.id === task.id)
  if (i >= 0) uploads.splice(i, 1)
}

// ---------- 条目操作 ----------
const downloadUrl = (e) => fsFileUrl('download', joinPath(currentPath.value, e.name))
const previewUrl = (e) => fsFileUrl('preview', joinPath(currentPath.value, e.name))

function canPreview(e) {
  return !e.is_dir && (isImageName(e.name) || isTextName(e.name))
}

function onEntryClick(e) {
  if (e.is_dir) go(joinPath(currentPath.value, e.name))
  else if (canPreview(e)) openPreview(e)
  else toast('该类型不支持页内预览, 请下载查看')
}

// 预览
const preview = reactive({ open: false, entry: null, kind: '', text: '', loading: false })

async function openPreview(e) {
  preview.open = true
  preview.entry = e
  preview.kind = isImageName(e.name) ? 'image' : 'text'
  preview.text = ''
  if (preview.kind === 'text') {
    preview.loading = true
    try {
      const resp = await fetch(previewUrl(e), { credentials: 'same-origin' })
      preview.text = resp.ok ? await resp.text() : `预览失败 (${resp.status})`
    } catch {
      preview.text = '预览加载失败'
    } finally {
      preview.loading = false
    }
  }
}

// 新建目录
const mkdir = reactive({ open: false, name: '', busy: false })

async function doMkdir() {
  const name = mkdir.name.trim()
  if (!name) return
  mkdir.busy = true
  try {
    await api.post('/api/fs/mkdir', { path: joinPath(currentPath.value, name) })
    mkdir.open = false
    mkdir.name = ''
    toast('目录已创建')
    refresh()
  } catch (e) {
    toast(e.message, 'error')
  } finally {
    mkdir.busy = false
  }
}

// 重命名
const rename = reactive({ open: false, entry: null, name: '', busy: false })

function openRename(e) {
  rename.entry = e
  rename.name = e.name
  rename.open = true
}

async function doRename() {
  const name = rename.name.trim()
  if (!name || name === rename.entry.name) {
    rename.open = false
    return
  }
  rename.busy = true
  try {
    await api.post('/api/fs/move', {
      from: joinPath(currentPath.value, rename.entry.name),
      to: joinPath(currentPath.value, name),
    })
    rename.open = false
    toast('已重命名')
    refresh()
  } catch (e) {
    toast(e.message, 'error')
  } finally {
    rename.busy = false
  }
}

// 移动 (目录选择器: 逐级浏览, 只列目录)
const move = reactive({ open: false, entry: null, target: '', dirs: [], busy: false })

async function loadMoveDirs() {
  try {
    const list = await api.get('/api/fs/list', { path: move.target })
    move.dirs = list.filter((x) => x.is_dir)
  } catch (e) {
    toast(e.message, 'error')
  }
}

function openMove(e) {
  move.entry = e
  move.target = ''
  move.open = true
  loadMoveDirs()
}

async function doMove() {
  const from = joinPath(currentPath.value, move.entry.name)
  const to = joinPath(move.target, move.entry.name)
  if (from === to || (move.entry.is_dir && (move.target === from || move.target.startsWith(from + '/')))) {
    toast('不能移动到原处或自身内部', 'error')
    return
  }
  move.busy = true
  try {
    await api.post('/api/fs/move', { from, to })
    move.open = false
    toast('已移动')
    refresh()
  } catch (e) {
    toast(e.message, 'error')
  } finally {
    move.busy = false
  }
}

// 删除 (目录与非空确认)
const del = reactive({ open: false, entry: null, busy: false })

function openDelete(e) {
  del.entry = e
  del.open = true
}

async function doDelete() {
  del.busy = true
  try {
    await api.post('/api/fs/delete', {
      path: joinPath(currentPath.value, del.entry.name),
      recursive: del.entry.is_dir,
    })
    del.open = false
    toast('已删除')
    refresh()
    refreshQuota()
  } catch (e) {
    toast(e.message, 'error')
  } finally {
    del.busy = false
  }
}

// 分享到图床
const shareImg = reactive({ open: false, entry: null, albums: [], albumId: '', busy: false, result: null })

async function openShareImages(e) {
  shareImg.entry = e
  shareImg.albumId = ''
  shareImg.result = null
  shareImg.open = true
  try {
    shareImg.albums = await api.get('/api/albums')
  } catch (err) {
    toast(err.message, 'error')
  }
}

async function doShareImages() {
  shareImg.busy = true
  try {
    const img = await api.post('/api/images/share', {
      path: joinPath(currentPath.value, shareImg.entry.name),
      album_id: shareImg.albumId || undefined,
    })
    shareImg.result = img
    toast(img.deduped ? '墙上已有这张图' : '已分享到图床')
  } catch (e) {
    toast(e.message, 'error')
  } finally {
    shareImg.busy = false
  }
}

// 分享到访客
const shareGuest = reactive({ open: false, result: null })

async function doShareGuest(e) {
  try {
    shareGuest.result = await api.post('/api/guest/share', {
      path: joinPath(currentPath.value, e.name),
    })
    shareGuest.open = true
    toast('已分享到访客空间')
  } catch (err) {
    toast(err.message, 'error')
  }
}
</script>

<template>
  <div class="page files">
    <!-- 面包屑 + 配额 -->
    <div class="files__top">
      <nav class="crumbs" aria-label="路径">
        <button class="crumbs__item" :class="{ 'crumbs__item--here': !currentPath }" @click="go('')">根目录</button>
        <template v-for="c in crumbs" :key="c.path">
          <span class="crumbs__sep">/</span>
          <button class="crumbs__item" :class="{ 'crumbs__item--here': c.path === currentPath }" @click="go(c.path)">
            {{ c.name }}
          </button>
        </template>
      </nav>
      <div v-if="quota" class="files__quota mono" :title="`磁盘剩余 ${fmtBytes(quota.disk.free)}`">
        <span>私有 {{ fmtBytes(quota.private.used) }} / {{ fmtBytes(quota.private.limit) }}</span>
        <span class="rail files__quota-rail">
          <span
            class="rail__fill"
            :style="{ '--p': quota.private.limit ? Math.min(quota.private.used / quota.private.limit, 1) : 0 }"
          />
        </span>
      </div>
    </div>

    <!-- 工具栏 -->
    <div class="files__bar">
      <button class="btn btn--primary" @click="pickFiles"><Icon name="upload" /> 上传</button>
      <button class="btn" @click="mkdir.open = true"><Icon name="plus" /> 新建目录</button>
      <label class="files__instant">
        <input v-model="instantTry" type="checkbox" />
        <span>秒传尝试</span>
      </label>
      <span class="files__hint muted">也可以直接把文件拖进来</span>
      <input ref="fileInput" type="file" multiple hidden @change="onPick" />
    </div>

    <!-- 上传任务 -->
    <div v-if="uploads.length" class="uploads card">
      <div v-for="t in uploads" :key="t.id" class="uploads__row">
        <span class="uploads__name" :title="t.name">{{ t.name }}</span>
        <span class="uploads__phase mono">
          <template v-if="t.phase === 'hash'">计算哈希 {{ Math.round(t.progress) }}%</template>
          <template v-else-if="t.phase === 'upload'">上传中 {{ Math.round(t.progress) }}%</template>
          <template v-else-if="t.phase === 'done'">完成</template>
          <template v-else>{{ t.error }}</template>
        </span>
        <span class="rail uploads__rail">
          <span
            class="rail__fill"
            :class="{ 'uploads__fill--error': t.phase === 'error' }"
            :style="{ '--p': (t.phase === 'done' ? 100 : t.progress) / 100 }"
          />
        </span>
        <button
          v-if="t.phase === 'hash' || t.phase === 'upload'"
          class="btn btn--ghost btn--small"
          @click="cancelUpload(t)"
        >
          取消
        </button>
        <button v-else class="btn btn--ghost btn--small" @click="dismissUpload(t)">
          <Icon name="close" :size="12" />
        </button>
      </div>
    </div>

    <!-- 列表 (拖拽区) -->
    <div
      class="files__list card"
      :class="{ 'files__list--drag': dragging }"
      @dragover.prevent="dragging = true"
      @dragleave.prevent="dragging = false"
      @drop.prevent="onDrop"
    >
      <div class="files__head mono">
        <span>名称</span>
        <span class="files__col-size">大小</span>
        <span class="files__col-time">修改时间</span>
        <span class="files__col-ops">操作</span>
      </div>
      <p v-if="error" class="files__empty">{{ error }}</p>
      <p v-else-if="!loading && !entries.length" class="files__empty">空目录 — 拖些文件进来吧</p>
      <div v-for="e in entries" :key="e.name" class="entry" @dblclick="onEntryClick(e)">
        <button class="entry__name" @click="onEntryClick(e)">
          <Icon :name="e.is_dir ? 'folder' : 'file'" class="entry__icon" :class="{ 'entry__icon--dir': e.is_dir }" />
          <span>{{ e.name }}</span>
        </button>
        <span class="files__col-size mono">{{ e.is_dir ? '—' : fmtBytes(e.size) }}</span>
        <span class="files__col-time mono">{{ fmtTime(e.modified) }}</span>
        <span class="files__col-ops">
          <template v-if="!e.is_dir">
            <a class="btn btn--ghost btn--small" :href="downloadUrl(e)" download title="下载"><Icon name="download" /></a>
            <button v-if="canPreview(e)" class="btn btn--ghost btn--small" title="预览" @click="openPreview(e)"><Icon name="eye" /></button>
            <button v-if="isImageName(e.name)" class="btn btn--ghost btn--small" title="分享到图床" @click="openShareImages(e)"><Icon name="image" /></button>
            <button class="btn btn--ghost btn--small" title="分享到访客" @click="doShareGuest(e)"><Icon name="share" /></button>
          </template>
          <button class="btn btn--ghost btn--small" title="重命名" @click="openRename(e)"><Icon name="pencil" /></button>
          <button class="btn btn--ghost btn--small" title="移动" @click="openMove(e)"><Icon name="move" /></button>
          <button class="btn btn--ghost btn--small btn--danger" title="删除" @click="openDelete(e)"><Icon name="trash" /></button>
        </span>
      </div>
    </div>

    <!-- 新建目录 -->
    <Modal v-if="mkdir.open" title="新建目录" @close="mkdir.open = false">
      <input v-model="mkdir.name" class="input" placeholder="目录名" autofocus @keyup.enter="doMkdir" />
      <template #footer>
        <button class="btn" @click="mkdir.open = false">取消</button>
        <button class="btn btn--primary" :disabled="mkdir.busy || !mkdir.name.trim()" @click="doMkdir">创建</button>
      </template>
    </Modal>

    <!-- 重命名 -->
    <Modal v-if="rename.open" title="重命名" @close="rename.open = false">
      <input v-model="rename.name" class="input" autofocus @keyup.enter="doRename" />
      <template #footer>
        <button class="btn" @click="rename.open = false">取消</button>
        <button class="btn btn--primary" :disabled="rename.busy || !rename.name.trim()" @click="doRename">确定</button>
      </template>
    </Modal>

    <!-- 移动 -->
    <Modal v-if="move.open" title="移动到" @close="move.open = false">
      <div class="movebox">
        <div class="movebox__where mono">
          <button class="movebox__crumb" @click="move.target = ''; loadMoveDirs()">根目录</button>
          <template v-for="(seg, i) in move.target ? move.target.split('/') : []" :key="i">
            <span>/</span>
            <button class="movebox__crumb" @click="move.target = (move.target.split('/').slice(0, i + 1).join('/')); loadMoveDirs()">{{ seg }}</button>
          </template>
        </div>
        <ul class="movebox__dirs">
          <li v-for="d in move.dirs" :key="d.name">
            <button class="movebox__dir" @click="move.target = joinPath(move.target, d.name); loadMoveDirs()">
              <Icon name="folder" /> {{ d.name }}
            </button>
          </li>
          <li v-if="!move.dirs.length" class="muted movebox__none">这一层没有子目录</li>
        </ul>
      </div>
      <template #footer>
        <button class="btn" @click="move.open = false">取消</button>
        <button class="btn btn--primary" :disabled="move.busy" @click="doMove">
          移到 {{ move.target ? `/${move.target}` : '根目录' }}
        </button>
      </template>
    </Modal>

    <!-- 删除确认 -->
    <Modal v-if="del.open" title="删除" @close="del.open = false">
      <p class="del__text">
        确定删除{{ del.entry?.is_dir ? '目录' : '文件' }}
        <strong>{{ del.entry?.name }}</strong> 吗?
        <template v-if="del.entry?.is_dir">目录里的内容会一起删除, </template>
        其他空间里对它的分享不受影响。
      </p>
      <template #footer>
        <button class="btn" @click="del.open = false">取消</button>
        <button class="btn btn--primary" :disabled="del.busy" @click="doDelete">删除</button>
      </template>
    </Modal>

    <!-- 分享到图床 -->
    <Modal v-if="shareImg.open" title="分享到图床" @close="shareImg.open = false">
      <template v-if="!shareImg.result">
        <p class="muted share__desc">把「{{ shareImg.entry?.name }}」挂到公开相册 (不复制文件, 只加一条指向)。</p>
        <label class="share__field mono">
          相册
          <select v-model="shareImg.albumId" class="input">
            <option value="">默认相册</option>
            <option v-for="a in shareImg.albums.filter((x) => !x.is_default)" :key="a.id" :value="a.id">{{ a.name }}</option>
          </select>
        </label>
      </template>
      <template v-else>
        <p class="share__desc">公开地址:</p>
        <div class="share__result">
          <input class="input mono" :value="shareImg.result.url" readonly @focus="$event.target.select()" />
          <button class="btn btn--small" @click="copyText(absoluteUrl(shareImg.result.url))">复制</button>
        </div>
      </template>
      <template #footer>
        <button class="btn" @click="shareImg.open = false">关闭</button>
        <button v-if="!shareImg.result" class="btn btn--primary" :disabled="shareImg.busy" @click="doShareImages">分享</button>
      </template>
    </Modal>

    <!-- 分享到访客结果 -->
    <Modal v-if="shareGuest.open" title="已分享到访客空间" @close="shareGuest.open = false">
      <LinkCard v-if="shareGuest.result" :url="shareGuest.result.url" :expires-at="shareGuest.result.expires_at" :name="shareGuest.result.orig_name" />
      <template #footer>
        <button class="btn btn--primary" @click="shareGuest.open = false">好</button>
      </template>
    </Modal>

    <!-- 预览 -->
    <Modal v-if="preview.open" :title="preview.entry?.name || '预览'" wide @close="preview.open = false">
      <div class="previewbox">
        <img v-if="preview.kind === 'image'" :src="previewUrl(preview.entry)" :alt="preview.entry?.name" class="previewbox__img" />
        <pre v-else class="previewbox__text">{{ preview.loading ? '加载中…' : preview.text }}</pre>
      </div>
      <template #footer>
        <a class="btn" :href="downloadUrl(preview.entry)" download>下载</a>
        <button class="btn btn--primary" @click="preview.open = false">关闭</button>
      </template>
    </Modal>
  </div>
</template>

<style scoped>
.files__top {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  flex-wrap: wrap;
  margin-bottom: 18px;
}

.crumbs {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 2px;
  font-size: 15px;
}

.crumbs__item {
  padding: 2px 4px;
  border: 0;
  background: none;
  color: var(--muted);
  cursor: pointer;
  transition: color .2s var(--ease-out);
}

.crumbs__item:hover {
  color: var(--red);
}

.crumbs__item--here {
  color: var(--ink);
  font-weight: 700;
}

.crumbs__sep {
  color: var(--soft);
}

.files__quota {
  display: flex;
  align-items: center;
  gap: 10px;
  color: var(--muted);
  white-space: nowrap;
}

.files__quota-rail {
  width: 110px;
}

.files__bar {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
  margin-bottom: 14px;
}

.files__instant {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 13px;
  color: var(--muted);
  cursor: pointer;
  user-select: none;
}

.files__instant input {
  accent-color: var(--red);
}

.files__hint {
  font-size: 12px;
  margin-left: auto;
}

.uploads {
  margin-bottom: 14px;
  padding: 6px 14px;
}

.uploads__row {
  display: grid;
  grid-template-columns: minmax(120px, 1fr) auto minmax(90px, 180px) auto;
  align-items: center;
  gap: 12px;
  padding: 7px 0;
  border-bottom: 1px solid var(--softer);
  font-size: 13px;
}

.uploads__row:last-child {
  border-bottom: 0;
}

.uploads__name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.uploads__phase {
  color: var(--muted);
  white-space: nowrap;
}

.uploads__fill--error {
  background: var(--muted);
}

.files__list {
  min-height: 200px;
  transition: border-color .2s, box-shadow .2s;
}

.files__list--drag {
  border-color: var(--red);
  box-shadow: inset 0 0 0 1px var(--red);
}

.files__head {
  display: grid;
  grid-template-columns: 1fr 90px 130px auto;
  gap: 12px;
  padding: 10px 16px;
  border-bottom: 1px solid var(--ink);
  color: var(--muted);
  text-transform: uppercase;
  letter-spacing: .08em;
}

.entry {
  display: grid;
  grid-template-columns: 1fr 90px 130px auto;
  gap: 12px;
  align-items: center;
  padding: 4px 16px;
  border-bottom: 1px solid var(--softer);
  transition: background .15s;
}

.entry:last-child {
  border-bottom: 0;
}

.entry:hover {
  background: rgba(24, 23, 18, .03);
}

.entry__name {
  display: flex;
  align-items: center;
  gap: 9px;
  padding: 6px 0;
  border: 0;
  background: none;
  text-align: left;
  font-size: 14px;
  cursor: pointer;
  min-width: 0;
}

.entry__name span {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.entry__name:hover span {
  color: var(--red);
}

.entry__icon {
  flex: none;
  color: var(--muted);
}

.entry__icon--dir {
  color: var(--ink);
}

.files__col-size,
.files__col-time {
  color: var(--muted);
  white-space: nowrap;
}

.files__col-ops {
  display: flex;
  gap: 2px;
  justify-self: end;
}

.files__empty {
  padding: 48px 16px;
  text-align: center;
  color: var(--muted);
  font-size: 14px;
}

.movebox__where {
  display: flex;
  align-items: center;
  gap: 4px;
  flex-wrap: wrap;
  color: var(--muted);
  margin-bottom: 10px;
}

.movebox__crumb {
  padding: 2px 4px;
  border: 0;
  background: none;
  color: var(--ink);
  font: inherit;
  cursor: pointer;
}

.movebox__crumb:hover {
  color: var(--red);
}

.movebox__dirs {
  max-height: 240px;
  overflow-y: auto;
  border: 1px solid var(--softer);
}

.movebox__dir {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 8px 12px;
  border: 0;
  border-bottom: 1px solid var(--softer);
  background: none;
  text-align: left;
  font-size: 14px;
  cursor: pointer;
}

.movebox__dir:hover {
  background: rgba(24, 23, 18, .04);
  color: var(--red);
}

.movebox__none {
  padding: 20px;
  text-align: center;
  font-size: 13px;
}

.del__text {
  font-size: 14px;
  line-height: 1.7;
}

.del__text strong {
  color: var(--red);
}

.share__desc {
  font-size: 14px;
  line-height: 1.7;
  margin-bottom: 14px;
}

.share__field {
  display: flex;
  flex-direction: column;
  gap: 6px;
  color: var(--muted);
  text-transform: uppercase;
  letter-spacing: .08em;
}

.share__result {
  display: flex;
  gap: 8px;
}

.share__result .input {
  font-size: 12px;
}

.previewbox {
  display: flex;
  justify-content: center;
}

.previewbox__img {
  max-height: 56vh;
  max-width: 100%;
  object-fit: contain;
}

.previewbox__text {
  width: 100%;
  max-height: 56vh;
  overflow: auto;
  margin: 0;
  padding: 14px;
  background: rgba(24, 23, 18, .04);
  border: 1px solid var(--softer);
  font: 12.5px/1.7 var(--font-mono);
  white-space: pre-wrap;
  word-break: break-all;
}

@media (max-width: 720px) {
  .files__head {
    display: none;
  }

  .entry {
    grid-template-columns: 1fr auto;
  }

  .files__col-size,
  .files__col-time {
    display: none;
  }
}
</style>
