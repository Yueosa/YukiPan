<script setup>
import { computed, onBeforeUnmount, onMounted, reactive, ref } from 'vue'
import { api } from '../api'
import { xhrUpload } from '../upload'
import { auth, toast, absoluteUrl, copyText } from '../store'
import { fmtBytes, fmtTime } from '../format'
import Icon from '../components/Icon.vue'
import Modal from '../components/Modal.vue'

// ---------- 数据 ----------
const albums = ref([])
const tags = ref([])
const activeTag = ref('')
const activeAlbum = ref(null) // null = 相册墙; 对象 = 进册看图
const images = ref([])
const total = ref(0)
const page = ref(0)
const loading = ref(false)
const wallError = ref('')

// 图片宽高比 (API 不带尺寸, 加载后记录, 用于 justified 布局)
const ratios = reactive({})
const PER_PAGE = 50
const ROW_H = 190

async function loadAlbums() {
  try {
    albums.value = await api.get('/api/albums')
  } catch (e) {
    toast(e.message, 'error')
  }
}

async function loadTags() {
  try {
    tags.value = await api.get('/api/tags')
  } catch {
    tags.value = []
  }
}

async function loadMore() {
  if (loading.value) return
  loading.value = true
  wallError.value = ''
  try {
    const data = await api.get('/api/images/list', {
      album_id: activeAlbum.value?.id,
      tag: activeTag.value || undefined,
      page: page.value + 1,
      per_page: PER_PAGE,
    })
    images.value.push(...data.items)
    total.value = data.total
    page.value = data.page
  } catch (e) {
    wallError.value = e.message
  } finally {
    loading.value = false
  }
}

function resetWall() {
  images.value = []
  page.value = 0
  total.value = 0
  loadMore()
}

function openAlbum(a) {
  activeAlbum.value = a
  resetWall()
}

function backToAlbums() {
  activeAlbum.value = null
  lightbox.open = false
  loadAlbums()
}

function pickTag(name) {
  activeTag.value = activeTag.value === name ? '' : name
  if (activeAlbum.value) resetWall()
}

function clearTag() {
  if (!activeTag.value) return
  activeTag.value = ''
  if (activeAlbum.value) resetWall()
}

onMounted(() => {
  loadAlbums()
  loadTags()
})

function onImgLoad(id, e) {
  const img = e.target
  if (img.naturalWidth && img.naturalHeight) ratios[id] = img.naturalWidth / img.naturalHeight
}

function cellStyle(img) {
  const r = ratios[img.id] || 1.5
  return { flexGrow: Math.round(r * ROW_H), flexBasis: `${Math.round(r * ROW_H)}px` }
}

// ---------- 灯箱 ----------
const lightbox = reactive({ open: false, index: 0 })
const current = computed(() => images.value[lightbox.index] || null)

function openLightbox(i) {
  lightbox.index = i
  lightbox.open = true
}

function step(d) {
  const n = images.value.length
  if (!n) return
  lightbox.index = (lightbox.index + d + n) % n
  // 翻到最后一张附近时预取下一页
  if (lightbox.index >= n - 3 && images.value.length < total.value) loadMore()
}

function onKey(e) {
  if (!lightbox.open) return
  if (e.key === 'Escape') lightbox.open = false
  else if (e.key === 'ArrowLeft') step(-1)
  else if (e.key === 'ArrowRight') step(1)
}

onMounted(() => document.addEventListener('keydown', onKey))
onBeforeUnmount(() => document.removeEventListener('keydown', onKey))

// ---------- 管理 (登录后) ----------
const admin = reactive({
  uploadOpen: false,
  files: [],
  albumId: '',
  tagsInput: '',
  busy: false,
  progress: -1, // -1 未开始
  albumOpen: false,
  albumName: '',
  tagEditOpen: false,
  tagEditValue: '',
  removeConfirm: false,
})
const uploadInput = ref(null)

function onPickUpload(e) {
  admin.files = Array.from(e.target.files || [])
  e.target.value = ''
}

async function doUploadImages() {
  if (!admin.files.length || admin.busy) return
  admin.busy = true
  admin.progress = 0
  const tags = admin.tagsInput.split(/[,，]/).map((s) => s.trim()).filter(Boolean)
  try {
    for (let i = 0; i < admin.files.length; i++) {
      const file = admin.files[i]
      await xhrUpload('/api/images/upload', file, {
        fields: {
          album_id: admin.albumId,
          tags: tags.length ? JSON.stringify(tags) : '',
        },
        onProgress: (loaded, t) => {
          admin.progress = ((i + (t ? loaded / t : 0)) / admin.files.length) * 100
        },
      }).promise
    }
    toast(`已上传 ${admin.files.length} 张`)
    admin.uploadOpen = false
    admin.files = []
    admin.tagsInput = ''
    loadAlbums()
    loadTags()
    if (activeAlbum.value) resetWall()
  } catch (e) {
    toast(e.message, 'error')
  } finally {
    admin.busy = false
    admin.progress = -1
  }
}

async function doCreateAlbum() {
  const name = admin.albumName.trim()
  if (!name) return
  try {
    await api.post('/api/albums', { name })
    toast('相册已创建')
    admin.albumOpen = false
    admin.albumName = ''
    loadAlbums()
  } catch (e) {
    toast(e.message, 'error')
  }
}

async function doDeleteAlbum(a) {
  try {
    await api.post('/api/albums/delete', { id: a.id })
    toast('相册已删除')
    loadAlbums()
  } catch (e) {
    toast(e.message, 'error')
  }
}

function openTagEdit() {
  admin.tagEditValue = (current.value?.tags || []).join(', ')
  admin.tagEditOpen = true
}

async function doSetTags() {
  const list = admin.tagEditValue.split(/[,，]/).map((s) => s.trim()).filter(Boolean)
  try {
    const tags = await api.post('/api/images/tags', { id: current.value.id, tags: list })
    current.value.tags = tags
    toast('标签已更新')
    admin.tagEditOpen = false
    loadTags()
  } catch (e) {
    toast(e.message, 'error')
  }
}

async function doRemoveImage() {
  try {
    await api.post('/api/images/delete', { id: current.value.id })
    toast('已从墙上拿掉 (私有文件不受影响)')
    admin.removeConfirm = false
    lightbox.open = false
    images.value.splice(lightbox.index, 1)
    total.value -= 1
    loadAlbums()
  } catch (e) {
    toast(e.message, 'error')
  }
}
</script>

<template>
  <div class="page photos">
    <!-- 标签筛选栏 -->
    <div v-if="tags.length" class="photos__tags">
      <button class="chip" :class="{ 'chip--active': !activeTag }" @click="clearTag">全部</button>
      <button
        v-for="t in tags"
        :key="t.id"
        class="chip"
        :class="{ 'chip--active': activeTag === t.name }"
        @click="pickTag(t.name)"
      >
        {{ t.name }} <span class="photos__tagcount">{{ t.count }}</span>
      </button>
    </div>

    <!-- 相册墙 -->
    <template v-if="!activeAlbum">
      <div class="photos__head">
        <h1 class="page-title">相册</h1>
        <div v-if="auth.user" class="photos__admin">
          <button class="btn" @click="admin.albumOpen = true"><Icon name="plus" /> 新建相册</button>
          <button class="btn btn--primary" @click="admin.uploadOpen = true"><Icon name="upload" /> 传图</button>
        </div>
      </div>
      <p v-if="!albums.length" class="photos__empty">还没有相册</p>
      <div class="albums">
        <article v-for="a in albums" :key="a.id" class="album card" @click="openAlbum(a)">
          <div class="album__cover">
            <img v-if="a.cover_url" :src="a.cover_url" :alt="a.name" loading="lazy" />
            <Icon v-else name="image" :size="30" class="album__placeholder" />
          </div>
          <div class="album__meta">
            <h2 class="album__name">{{ a.name }}</h2>
            <span class="mono album__count">{{ a.image_count }} 张</span>
            <button
              v-if="auth.user && !a.is_default && a.image_count === 0"
              class="btn btn--ghost btn--small album__del"
              title="删除空相册"
              @click.stop="doDeleteAlbum(a)"
            >
              <Icon name="trash" :size="13" />
            </button>
          </div>
        </article>
      </div>
    </template>

    <!-- 图片流 -->
    <template v-else>
      <div class="photos__head">
        <button class="btn btn--ghost" @click="backToAlbums"><Icon name="back" /> 相册</button>
        <h1 class="page-title">{{ activeAlbum.name }}</h1>
        <span class="mono muted">{{ total }} 张</span>
        <div v-if="auth.user" class="photos__admin">
          <button class="btn btn--primary" @click="admin.albumId = activeAlbum.id; admin.uploadOpen = true"><Icon name="upload" /> 传图</button>
        </div>
      </div>

      <p v-if="wallError" class="photos__empty">{{ wallError }}</p>
      <p v-else-if="!loading && !images.length" class="photos__empty">这个相册还是空的</p>
      <div class="wall">
        <figure v-for="(img, i) in images" :key="img.id" class="wall__cell" :style="cellStyle(img)">
          <img :src="img.url" :alt="img.orig_name" loading="lazy" @load="onImgLoad(img.id, $event)" @click="openLightbox(i)" />
        </figure>
      </div>
      <div class="photos__more">
        <button v-if="images.length < total" class="btn" :disabled="loading" @click="loadMore">
          {{ loading ? '加载中…' : '加载更多' }}
        </button>
      </div>
    </template>

    <!-- 灯箱 -->
    <transition name="fade">
      <div v-if="lightbox.open && current" class="lightbox" @click.self="lightbox.open = false">
        <button class="lightbox__close btn btn--ghost" @click="lightbox.open = false"><Icon name="close" /></button>
        <button class="lightbox__nav lightbox__nav--prev" @click="step(-1)"><Icon name="left" :size="22" /></button>
        <figure class="lightbox__stage">
          <img :src="current.url" :alt="current.orig_name" />
          <figcaption class="lightbox__caption">
            <span class="lightbox__name">{{ current.orig_name }}</span>
            <span class="mono muted">{{ fmtBytes(current.size) }} · {{ fmtTime(current.created_at) }} · {{ current.album_name }}</span>
            <span v-if="current.tags.length" class="lightbox__tags">
              <span v-for="t in current.tags" :key="t" class="chip chip--active">{{ t }}</span>
            </span>
            <span class="lightbox__ops">
              <button class="btn btn--small" @click="copyText(absoluteUrl(current.url))"><Icon name="copy" :size="13" /> 复制链接</button>
              <template v-if="auth.user">
                <button class="btn btn--small" @click="openTagEdit"><Icon name="tag" :size="13" /> 标签</button>
                <button class="btn btn--small btn--danger" @click="admin.removeConfirm = true"><Icon name="trash" :size="13" /> 从墙上拿掉</button>
              </template>
            </span>
          </figcaption>
        </figure>
        <button class="lightbox__nav lightbox__nav--next" @click="step(1)"><Icon name="right" :size="22" /></button>
      </div>
    </transition>

    <!-- 传图 -->
    <Modal v-if="admin.uploadOpen" title="传图" @close="admin.uploadOpen = false">
      <div class="upform">
        <button class="btn" @click="uploadInput?.click()"><Icon name="image" /> 选择图片</button>
        <input ref="uploadInput" type="file" accept=".jpg,.jpeg,.png,.gif,.webp,.avif" multiple hidden @change="onPickUpload" />
        <p v-if="admin.files.length" class="muted upform__files">
          已选 {{ admin.files.length }} 张: {{ admin.files.map((f) => f.name).slice(0, 3).join('、') }}{{ admin.files.length > 3 ? ' …' : '' }}
        </p>
        <label class="upform__field mono">
          相册
          <select v-model="admin.albumId" class="input">
            <option value="">默认相册</option>
            <option v-for="a in albums.filter((x) => !x.is_default)" :key="a.id" :value="a.id">{{ a.name }}</option>
          </select>
        </label>
        <label class="upform__field mono">
          标签 (逗号分隔, 可空)
          <input v-model="admin.tagsInput" class="input" placeholder="风景, 旅行" />
        </label>
        <span v-if="admin.progress >= 0" class="rail"><span class="rail__fill" :style="{ '--p': admin.progress / 100 }" /></span>
      </div>
      <template #footer>
        <button class="btn" @click="admin.uploadOpen = false">取消</button>
        <button class="btn btn--primary" :disabled="!admin.files.length || admin.busy" @click="doUploadImages">
          {{ admin.busy ? '上传中…' : '上传' }}
        </button>
      </template>
    </Modal>

    <!-- 新建相册 -->
    <Modal v-if="admin.albumOpen" title="新建相册" @close="admin.albumOpen = false">
      <input v-model="admin.albumName" class="input" placeholder="相册名" autofocus @keyup.enter="doCreateAlbum" />
      <template #footer>
        <button class="btn" @click="admin.albumOpen = false">取消</button>
        <button class="btn btn--primary" :disabled="!admin.albumName.trim()" @click="doCreateAlbum">创建</button>
      </template>
    </Modal>

    <!-- 编辑标签 -->
    <Modal v-if="admin.tagEditOpen" title="编辑标签" @close="admin.tagEditOpen = false">
      <input v-model="admin.tagEditValue" class="input" placeholder="逗号分隔; 清空即移除全部标签" autofocus @keyup.enter="doSetTags" />
      <template #footer>
        <button class="btn" @click="admin.tagEditOpen = false">取消</button>
        <button class="btn btn--primary" @click="doSetTags">保存</button>
      </template>
    </Modal>

    <!-- 拿掉确认 -->
    <Modal v-if="admin.removeConfirm" title="从墙上拿掉" @close="admin.removeConfirm = false">
      <p class="photos__confirm">把「{{ current?.orig_name }}」从图床拿掉? 只是删掉这条指向, 私有区的文件还在。</p>
      <template #footer>
        <button class="btn" @click="admin.removeConfirm = false">取消</button>
        <button class="btn btn--primary" @click="doRemoveImage">拿掉</button>
      </template>
    </Modal>
  </div>
</template>

<style scoped>
.photos__tags {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
  margin-bottom: 22px;
}

.photos__tagcount {
  margin-left: 4px;
  opacity: .6;
  font-size: 10px;
}

.photos__head {
  display: flex;
  align-items: baseline;
  gap: 14px;
  margin-bottom: 22px;
  flex-wrap: wrap;
}

.photos__admin {
  margin-left: auto;
  display: flex;
  gap: 10px;
}

.photos__empty {
  padding: 60px 0;
  text-align: center;
  color: var(--muted);
}

/* 相册墙 */
.albums {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
  gap: 18px;
}

.album {
  cursor: pointer;
  overflow: hidden;
  transition: transform .3s var(--ease-out), box-shadow .3s var(--ease-out);
}

.album:hover {
  transform: translateY(-3px);
  box-shadow: 0 12px 28px rgba(24, 23, 18, .12);
}

.album__cover {
  aspect-ratio: 4 / 3;
  background: var(--softer);
  display: flex;
  align-items: center;
  justify-content: center;
  overflow: hidden;
}

.album__cover img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  transition: transform .5s var(--ease-out);
}

.album:hover .album__cover img {
  transform: scale(1.04);
}

.album__placeholder {
  color: var(--muted);
  opacity: .5;
}

.album__meta {
  display: flex;
  align-items: baseline;
  gap: 10px;
  padding: 12px 14px;
}

.album__name {
  font: 700 1rem var(--font-display);
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.album__count {
  color: var(--muted);
}

.album__del {
  align-self: center;
}

/* 图片流: 等高行, 宽度按宽高比分配 (flex-grow ∝ 宽高比, 不引库) */
.wall {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}

.wall::after {
  content: '';
  flex-grow: 1000000;
}

.wall__cell {
  position: relative;
  height: 190px;
  max-width: 100%;
  overflow: hidden;
  background: var(--softer);
  cursor: zoom-in;
}

.wall__cell img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  transition: transform .4s var(--ease-out), filter .4s var(--ease-out);
}

.wall__cell:hover img {
  transform: scale(1.03);
  filter: brightness(1.04);
}

.photos__more {
  display: flex;
  justify-content: center;
  padding: 28px 0;
}

/* 灯箱 */
.lightbox {
  position: fixed;
  inset: 0;
  z-index: 70;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 10px;
  padding: 40px 16px;
  background: rgba(17, 16, 15, .92);
}

.lightbox__stage {
  max-width: min(1100px, 86vw);
  max-height: 100%;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 12px;
}

.lightbox__stage img {
  max-height: 72vh;
  max-width: 100%;
  object-fit: contain;
  box-shadow: 0 20px 60px rgba(0, 0, 0, .5);
}

.lightbox__caption {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
  color: #d7d0c3;
  text-align: center;
}

.lightbox__name {
  font-size: 14px;
}

.lightbox__caption .muted {
  color: #8d8680;
}

.lightbox__tags {
  display: flex;
  gap: 6px;
}

.lightbox__ops {
  display: flex;
  gap: 8px;
  margin-top: 4px;
}

.lightbox__ops .btn {
  border-color: #4a4540;
  color: #d7d0c3;
}

.lightbox__ops .btn:hover {
  border-color: var(--red);
  color: var(--red);
}

.lightbox__close {
  position: absolute;
  top: 14px;
  right: 14px;
  color: #d7d0c3;
}

.lightbox__nav {
  flex: none;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 44px;
  height: 44px;
  border: 1px solid #4a4540;
  border-radius: 50%;
  background: none;
  color: #d7d0c3;
  cursor: pointer;
  transition: all .25s var(--ease-out);
}

.lightbox__nav:hover {
  border-color: var(--red);
  color: var(--red);
}

/* 传图表单 */
.upform {
  display: flex;
  flex-direction: column;
  gap: 14px;
}

.upform__files {
  font-size: 13px;
}

.upform__field {
  display: flex;
  flex-direction: column;
  gap: 6px;
  color: var(--muted);
  text-transform: uppercase;
  letter-spacing: .08em;
}

.photos__confirm {
  font-size: 14px;
  line-height: 1.7;
}

@media (max-width: 720px) {
  .wall__cell {
    height: 130px;
  }

  .lightbox__nav {
    position: absolute;
    bottom: 18px;
  }

  .lightbox__nav--prev {
    left: 18px;
  }

  .lightbox__nav--next {
    right: 18px;
  }
}
</style>
