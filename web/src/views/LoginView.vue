<script setup>
import { ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { api } from '../api'
import { auth } from '../store'

const route = useRoute()
const router = useRouter()

const username = ref('')
const password = ref('')
const error = ref('')
const busy = ref(false)

async function submit() {
  if (busy.value) return
  error.value = ''
  busy.value = true
  try {
    auth.user = await api.post('/api/auth/login', {
      username: username.value.trim(),
      password: password.value,
    })
    const redirect = typeof route.query.redirect === 'string' ? route.query.redirect : '/files'
    router.push(redirect.startsWith('/') ? redirect : '/files')
  } catch (e) {
    // 后端不区分用户名/密码错误, message 原样展示
    error.value = e.message
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <div class="login">
    <form class="login__card card" @submit.prevent="submit">
      <h1 class="login__brand">YukiPan</h1>
      <p class="login__sub mono">私人网盘 · 登录</p>

      <label class="login__field">
        <span class="mono">用户名</span>
        <input v-model="username" class="input" type="text" autocomplete="username" required autofocus />
      </label>
      <label class="login__field">
        <span class="mono">密码</span>
        <input v-model="password" class="input" type="password" autocomplete="current-password" required />
      </label>

      <p v-if="error" class="login__error">{{ error }}</p>

      <button class="btn btn--primary login__submit" type="submit" :disabled="busy">
        {{ busy ? '登录中…' : '登录' }}
      </button>
    </form>
  </div>
</template>

<style scoped>
.login {
  display: flex;
  justify-content: center;
  padding: 10vh 16px 40px;
}

.login__card {
  width: min(360px, 100%);
  padding: 36px 32px 32px;
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.login__brand {
  font: 900 2rem var(--font-display);
  letter-spacing: -.04em;
  text-align: center;
}

.login__sub {
  text-align: center;
  color: var(--muted);
  text-transform: uppercase;
  letter-spacing: .14em;
  margin-top: -10px;
}

.login__field {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.login__field .mono {
  color: var(--muted);
  text-transform: uppercase;
  letter-spacing: .1em;
}

.login__error {
  font-size: 13px;
  color: var(--red);
  text-align: center;
}

.login__submit {
  justify-content: center;
  padding: 10px;
  font-size: 14px;
}
</style>
