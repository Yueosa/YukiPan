// 统一 API 客户端: 解 { success, data, message } 信封, 401 走注册的回调 (清登录态/跳登录),
// 其余错误 (429/413/409...) 透出后端 message。

export class ApiError extends Error {
  constructor(status, message) {
    super(message)
    this.status = status
  }
}

let unauthorizedHandler = () => {}

export function setUnauthorizedHandler(fn) {
  unauthorizedHandler = fn
}

async function request(method, path, { query, body, skipAuthRedirect } = {}) {
  let qs = ''
  if (query) {
    const params = new URLSearchParams()
    for (const [k, v] of Object.entries(query)) {
      if (v !== undefined && v !== null && v !== '') params.set(k, v)
    }
    const s = params.toString()
    if (s) qs = `?${s}`
  }
  let resp
  try {
    resp = await fetch(path + qs, {
      method,
      credentials: 'same-origin',
      headers: body !== undefined ? { 'Content-Type': 'application/json' } : undefined,
      body: body !== undefined ? JSON.stringify(body) : undefined,
    })
  } catch {
    throw new ApiError(0, '网络错误, 请稍后再试')
  }
  let env = null
  try {
    env = await resp.json()
  } catch {
    // 非 JSON 响应 (理论上 JSON 口不会走到)
  }
  if (resp.ok && env && env.success) return env.data
  const message = (env && env.message) || `请求失败 (${resp.status})`
  if (resp.status === 401 && !skipAuthRedirect) unauthorizedHandler()
  throw new ApiError(resp.status, message)
}

export const api = {
  get: (path, query, opts) => request('GET', path, { query, ...opts }),
  post: (path, body, opts) => request('POST', path, { body, ...opts }),
}

// 私有区下载/预览地址: dev 环境无 nginx, 带 direct=1 让后端直接出文件; 生产走 X-Accel。
export function fsFileUrl(kind, path) {
  const params = new URLSearchParams({ path })
  if (import.meta.env.DEV) params.set('direct', '1')
  return `/api/fs/${kind}?${params.toString()}`
}
