import { createRouter, createWebHistory } from 'vue-router'
import { auth, initAuth } from './store'

const routes = [
  { path: '/', redirect: '/files' },
  { path: '/login', component: () => import('./views/LoginView.vue'), meta: { public: true } },
  { path: '/files', component: () => import('./views/FilesView.vue') },
  { path: '/photos', component: () => import('./views/PhotosView.vue'), meta: { public: true } },
  { path: '/guest', component: () => import('./views/GuestView.vue'), meta: { public: true } },
  { path: '/:pathMatch(.*)*', redirect: '/files' },
]

export const router = createRouter({
  history: createWebHistory(),
  routes,
})

// 私有区守卫: 未登录一律跳登录 (调 /api/auth/me 确认会话); 公开页直接放行。
router.beforeEach(async (to) => {
  if (to.meta.public) return true
  if (!auth.ready) await initAuth()
  if (!auth.user) {
    return { path: '/login', query: to.fullPath !== '/files' ? { redirect: to.fullPath } : {} }
  }
  return true
})
