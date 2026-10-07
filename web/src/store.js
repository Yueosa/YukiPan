// 登录态与全局提示 (轻量 reactive store, 不引状态库)
import { reactive } from 'vue'
import { api } from './api'

export const auth = reactive({
  user: null,
  ready: false,
})

// 路由守卫首次进受保护页时调用; 401 属正常 (未登录), 不触发跳登录回调。
export async function initAuth() {
  try {
    auth.user = await api.get('/api/auth/me', undefined, { skipAuthRedirect: true })
  } catch {
    auth.user = null
  } finally {
    auth.ready = true
  }
}

export async function logout() {
  try {
    await api.post('/api/auth/logout')
  } catch {
    // 有没有会话都算退出
  }
  auth.user = null
}

// ---------- toast ----------
let seq = 0
export const toasts = reactive([])

export function toast(message, kind = 'info', duration) {
  const id = ++seq
  toasts.push({ id, message, kind })
  const ttl = duration ?? (kind === 'error' ? 6000 : 3500)
  setTimeout(() => {
    const i = toasts.findIndex((t) => t.id === id)
    if (i >= 0) toasts.splice(i, 1)
  }, ttl)
}

export async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text)
    toast('已复制到剪贴板')
  } catch {
    // 剪贴板 API 不可用 (非安全上下文) 时退到 execCommand
    const ta = document.createElement('textarea')
    ta.value = text
    document.body.appendChild(ta)
    ta.select()
    document.execCommand('copy')
    ta.remove()
    toast('已复制到剪贴板')
  }
}

export function absoluteUrl(path) {
  return `${location.origin}${path}`
}
