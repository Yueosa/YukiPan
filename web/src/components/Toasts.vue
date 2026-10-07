<script setup>
import { toasts } from '../store'
import Icon from './Icon.vue'
</script>

<template>
  <div class="toasts" aria-live="polite">
    <transition-group name="pop">
      <div v-for="t in toasts" :key="t.id" class="toast card" :class="`toast--${t.kind}`">
        <Icon v-if="t.kind === 'error'" name="close" :size="13" />
        <Icon v-else name="check" :size="13" />
        <span>{{ t.message }}</span>
      </div>
    </transition-group>
  </div>
</template>

<style scoped>
.toasts {
  position: fixed;
  right: 20px;
  bottom: 20px;
  z-index: 90;
  display: flex;
  flex-direction: column;
  gap: 8px;
  max-width: min(380px, 90vw);
}

.toast {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 10px 14px;
  font-size: 13px;
  border-left: 2px solid var(--ink);
  box-shadow: 0 8px 24px rgba(24, 23, 18, .14);
}

.toast--error {
  border-left-color: var(--red);
  color: var(--red);
}
</style>
