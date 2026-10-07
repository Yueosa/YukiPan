import { createApp } from 'vue'
import App from './App.vue'
import { router } from './router'
import { setUnauthorizedHandler } from './api'
import { auth } from './store'
import './styles/base.css'

// API 层 401: 清登录态; 若当前在受保护页, 跳登录
setUnauthorizedHandler(() => {
  auth.user = null
  const r = router.currentRoute.value
  if (r && r.meta && !r.meta.public && r.path !== '/login') {
    router.replace({ path: '/login', query: { redirect: r.fullPath } })
  }
})

createApp(App).use(router).mount('#app')
