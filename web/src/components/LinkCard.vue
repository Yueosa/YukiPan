<script setup>
// 链接卡片: 访客上传成功 / 分享到访客 后展示 — 链接 + 一键复制 + 过期倒计时 + 醒目提醒
import { computed } from 'vue'
import Icon from './Icon.vue'
import Countdown from './Countdown.vue'
import { absoluteUrl, copyText } from '../store'
import { fmtTime } from '../format'

const props = defineProps({
  url: { type: String, required: true },
  expiresAt: { type: String, required: true },
  name: { type: String, default: '' },
})

const fullUrl = computed(() => absoluteUrl(props.url))
</script>

<template>
  <div class="linkcard card">
    <div class="linkcard__row">
      <Icon name="link" class="linkcard__icon" />
      <input class="input mono linkcard__url" :value="fullUrl" readonly @focus="$event.target.select()" />
      <button class="btn btn--primary btn--small" @click="copyText(fullUrl)">
        <Icon name="copy" :size="13" /> 复制
      </button>
    </div>
    <div class="linkcard__meta">
      <span v-if="name" class="linkcard__name">{{ name }}</span>
      <span class="linkcard__count">
        <Icon name="clock" :size="12" />
        <Countdown :expires-at="expiresAt" />
        <span class="linkcard__at">({{ fmtTime(expiresAt) }})</span>
      </span>
    </div>
    <p class="linkcard__warn">关闭页面前请保存链接 — 没有公开列表可以找回它。</p>
  </div>
</template>

<style scoped>
.linkcard {
  padding: 14px 16px;
  border-left: 2px solid var(--red);
}

.linkcard__row {
  display: flex;
  align-items: center;
  gap: 10px;
}

.linkcard__icon {
  flex: none;
  color: var(--muted);
}

.linkcard__url {
  flex: 1;
  font-size: 12px;
}

.linkcard__meta {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  margin-top: 10px;
  flex-wrap: wrap;
}

.linkcard__name {
  font-size: 13px;
  color: var(--muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 50%;
}

.linkcard__count {
  display: inline-flex;
  align-items: center;
  gap: 5px;
}

.linkcard__at {
  font: 12px var(--font-mono);
  color: var(--muted);
}

.linkcard__warn {
  margin-top: 8px;
  font-size: 12px;
  color: var(--red);
}
</style>
