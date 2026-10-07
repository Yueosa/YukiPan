// 展示格式化小件

export function fmtBytes(n) {
  if (n === undefined || n === null) return '-'
  if (n < 1024) return `${n} B`
  const units = ['KB', 'MB', 'GB', 'TB']
  let v = n
  let u = -1
  do {
    v /= 1024
    u++
  } while (v >= 1024 && u < units.length - 1)
  return `${v >= 100 ? Math.round(v) : v.toFixed(1)} ${units[u]}`
}

export function fmtTime(iso) {
  if (!iso) return '-'
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return '-'
  const p = (x) => String(x).padStart(2, '0')
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`
}

// 剩余毫秒 → HH:MM:SS
export function fmtCountdown(ms) {
  if (ms <= 0) return '00:00:00'
  const s = Math.floor(ms / 1000)
  const p = (x) => String(x).padStart(2, '0')
  return `${p(Math.floor(s / 3600))}:${p(Math.floor((s % 3600) / 60))}:${p(s % 60)}`
}

export function extOf(name) {
  const i = name.lastIndexOf('.')
  return i > 0 ? name.slice(i + 1).toLowerCase() : ''
}

const IMAGE_EXTS = ['jpg', 'jpeg', 'png', 'gif', 'webp', 'avif']
const TEXT_EXTS = ['txt', 'md', 'json', 'log', 'csv', 'xml', 'yaml', 'yml', 'toml', 'ini', 'conf', 'js', 'css', 'html', 'svg', 'sh']

export function isImageName(name) {
  return IMAGE_EXTS.includes(extOf(name))
}

export function isTextName(name) {
  return TEXT_EXTS.includes(extOf(name))
}

export function joinPath(dir, name) {
  return dir ? `${dir}/${name}` : name
}

export function parentPath(path) {
  const i = path.lastIndexOf('/')
  return i < 0 ? '' : path.slice(0, i)
}
