<script setup>
import { useRouter } from 'vue-router'
import { auth, logout } from '../store'

const router = useRouter()

async function onLogout() {
  await logout()
  const r = router.currentRoute.value
  if (r.meta && !r.meta.public) {
    router.push('/login')
  }
}
</script>

<template>
  <header class="topbar">
    <router-link to="/" class="topbar__brand">YukiPan</router-link>
    <nav class="topbar__nav">
      <router-link to="/files" class="topbar__link" active-class="topbar__link--active">文件</router-link>
      <router-link to="/photos" class="topbar__link" active-class="topbar__link--active">图床</router-link>
      <router-link to="/guest" class="topbar__link" active-class="topbar__link--active">访客</router-link>
    </nav>
    <div class="topbar__side mono">
      <template v-if="auth.user">
        <span class="topbar__user">{{ auth.user.username }}</span>
        <button class="topbar__action" @click="onLogout">退出</button>
      </template>
      <router-link v-else-if="$route.path !== '/login'" to="/login" class="topbar__action">登录</router-link>
    </div>
  </header>
</template>

<style scoped>
.topbar {
  position: sticky;
  top: 0;
  z-index: 40;
  display: grid;
  grid-template-columns: 1fr auto 1fr;
  align-items: center;
  padding: 0 3vw;
  height: var(--topbar-height);
  border-bottom: 1px solid var(--ink);
  background: var(--paper);
}

.topbar__brand {
  font: 900 1.3rem var(--font-display);
  letter-spacing: -.04em;
  transition: color .25s var(--ease-out);
}

.topbar__brand:hover {
  color: var(--red);
}

.topbar__nav {
  display: flex;
  gap: 26px;
}

.topbar__link {
  position: relative;
  padding: 4px 2px;
  font-size: 13px;
  color: var(--muted);
  transition: color .25s var(--ease-out);
}

.topbar__link:hover {
  color: var(--ink);
}

.topbar__link--active {
  color: var(--ink);
}

.topbar__link--active::after {
  content: '';
  position: absolute;
  left: 0;
  right: 0;
  bottom: -2px;
  height: 2px;
  background: var(--red);
}

.topbar__side {
  justify-self: end;
  display: flex;
  align-items: center;
  gap: 12px;
  color: var(--muted);
  text-transform: uppercase;
}

.topbar__user {
  letter-spacing: .06em;
}

.topbar__action {
  padding: 0;
  border: 0;
  background: none;
  color: var(--ink);
  font: inherit;
  text-transform: inherit;
  cursor: pointer;
  transition: color .25s var(--ease-out);
}

.topbar__action:hover {
  color: var(--red);
}

@media (max-width: 720px) {
  .topbar {
    grid-template-columns: auto 1fr auto;
    gap: 14px;
  }

  .topbar__nav {
    gap: 16px;
    justify-self: center;
  }
}
</style>
