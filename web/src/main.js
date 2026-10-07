import { createApp } from 'vue'
import App from './App.vue'
import { router } from './router'
import { setUnauthorizedHandler } from './api'
import { auth, initAuth } from './store'
import './styles/base.css'

// API 层 401: 清登录态; 若当前在受保护页, 跳登录
setUnauthorizedHandler(() => {
  auth.user = null
  const r = router.currentRoute.value
  if (r && r.meta && !r.meta.public && r.path !== '/login') {
    router.replace({ path: '/login', query: { redirect: r.fullPath } })
  }
})

// 公开页也需要知道登录态 (图床/访客的管理功能浮出), 启动即探测一次, 结果响应式填入
initAuth()

createApp(App).use(router).mount('#app')
