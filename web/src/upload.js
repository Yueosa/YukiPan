// XHR 上传: fetch 没有上传进度, 用 XMLHttpRequest; 返回 { promise, abort } 支持取消。
import { ApiError } from './api'

export function xhrUpload(url, file, { onProgress, fields, headers } = {}) {
  const xhr = new XMLHttpRequest()
  const promise = new Promise((resolve, reject) => {
    xhr.open('POST', url)
    if (headers) {
      for (const [k, v] of Object.entries(headers)) {
        if (v !== undefined && v !== null && v !== '') xhr.setRequestHeader(k, v)
      }
    }
    xhr.upload.onprogress = (e) => {
      if (e.lengthComputable && onProgress) onProgress(e.loaded, e.total)
    }
    xhr.onload = () => {
      let env = null
      try {
        env = JSON.parse(xhr.responseText)
      } catch {
        // 非 JSON (网关错误等)
      }
      if (xhr.status >= 200 && xhr.status < 300 && env && env.success) {
        resolve(env.data)
      } else {
        reject(new ApiError(xhr.status, (env && env.message) || `上传失败 (${xhr.status})`))
      }
    }
    xhr.onerror = () => reject(new ApiError(0, '网络错误, 上传中断'))
    xhr.onabort = () => reject(new ApiError(0, '已取消'))
    const fd = new FormData()
    // file 字段在前: 图床后端在 file 之后读可选文本字段 (album_id/tags)
    fd.append('file', file)
    if (fields) {
      for (const [k, v] of Object.entries(fields)) {
        if (v !== undefined && v !== null && v !== '') fd.append(k, v)
      }
    }
    xhr.send(fd)
  })
  return { promise, abort: () => xhr.abort() }
}
