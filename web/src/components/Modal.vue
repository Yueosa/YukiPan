<script setup>
// 通用弹窗: 点击遮罩 / Esc 关闭
import { onMounted, onBeforeUnmount } from 'vue'
import Icon from './Icon.vue'

const props = defineProps({
  title: { type: String, default: '' },
  wide: { type: Boolean, default: false },
})
const emit = defineEmits(['close'])

function onKey(e) {
  if (e.key === 'Escape') emit('close')
}

onMounted(() => document.addEventListener('keydown', onKey))
onBeforeUnmount(() => document.removeEventListener('keydown', onKey))
</script>

<template>
  <transition name="fade">
    <div class="modal-mask" @click.self="emit('close')">
      <transition name="pop" appear>
        <div class="modal card" :class="{ 'modal--wide': wide }" role="dialog" :aria-label="title">
          <header class="modal__head">
            <h3>{{ title }}</h3>
            <button class="btn btn--ghost btn--small" @click="emit('close')" aria-label="关闭">
              <Icon name="close" />
            </button>
          </header>
          <div class="modal__body">
            <slot />
          </div>
          <footer v-if="$slots.footer" class="modal__foot">
            <slot name="footer" />
          </footer>
        </div>
      </transition>
    </div>
  </transition>
</template>

<style scoped>
.modal-mask {
  position: fixed;
  inset: 0;
  z-index: 60;
  display: flex;
  align-items: flex-start;
  justify-content: center;
  padding: 12vh 16px 16px;
  background: rgba(24, 23, 18, .38);
  backdrop-filter: blur(2px);
}

.modal {
  width: min(460px, 100%);
  max-height: 76vh;
  display: flex;
  flex-direction: column;
  box-shadow: 0 18px 50px rgba(24, 23, 18, .22);
}

.modal--wide {
  width: min(760px, 100%);
}

.modal__head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 16px 18px 12px;
  border-bottom: 1px solid var(--softer);
}

.modal__head h3 {
  font: 700 1.05rem var(--font-display);
  letter-spacing: -.01em;
}

.modal__body {
  padding: 18px;
  overflow-y: auto;
}

.modal__foot {
  display: flex;
  justify-content: flex-end;
  gap: 10px;
  padding: 12px 18px 16px;
  border-top: 1px solid var(--softer);
}
</style>
