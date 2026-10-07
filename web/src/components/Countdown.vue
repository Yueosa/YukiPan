<script setup>
// 实时倒计时: 到 expiresAt 的剩余时间 (HH:MM:SS), 过期显示「已过期」。
// 从 LinkCard 抽出的 tick 逻辑, LinkCard 与访客管理列表共用。
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { fmtCountdown } from '../format'

const props = defineProps({
  expiresAt: { type: String, required: true },
})

const now = ref(Date.now())
let timer = null
onMounted(() => {
  timer = setInterval(() => {
    now.value = Date.now()
  }, 1000)
})
onBeforeUnmount(() => clearInterval(timer))

const remainMs = computed(() => new Date(props.expiresAt).getTime() - now.value)
const expired = computed(() => remainMs.value <= 0)
</script>

<template>
  <span class="countdown" :class="{ 'countdown--expired': expired }">
    <template v-if="!expired">{{ fmtCountdown(remainMs) }}</template>
    <template v-else>已过期</template>
  </span>
</template>

<style scoped>
.countdown {
  font: 12px var(--font-mono);
  color: var(--red);
  white-space: nowrap;
}

.countdown--expired {
  color: var(--muted);
}
</style>
