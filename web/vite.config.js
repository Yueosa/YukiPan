import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import fs from 'node:fs'
import path from 'node:path'

// 后端地址 (dev)。可用 YUKIPAN_BACKEND 覆盖。
const BACKEND = process.env.YUKIPAN_BACKEND || 'http://127.0.0.1:18516'
// dev 数据根: 生产里 /public 由 nginx 直出, 后端不出静态文件;
// vite dev 没有 nginx, 用一个 dev-only 中间件从数据根出 /public。
// 用 YUKIPAN_DATA_ROOT 指向起后端时用的 data_root。
const DATA_ROOT = process.env.YUKIPAN_DATA_ROOT || '/tmp/yukipan-dev'

const MIME = {
  jpg: 'image/jpeg', jpeg: 'image/jpeg', png: 'image/png', gif: 'image/gif',
  webp: 'image/webp', avif: 'image/avif', pdf: 'application/pdf',
  zip: 'application/zip', '7z': 'application/x-7z-compressed',
  txt: 'text/plain; charset=utf-8', md: 'text/plain; charset=utf-8',
}

function publicFromDataRoot() {
  return (req, res, next) => {
    if (!req.url || !req.url.startsWith('/public/')) return next()
    const rel = decodeURIComponent(req.url.split('?')[0].slice('/public/'.length))
    const file = path.join(DATA_ROOT, 'public', rel)
    // 防穿越: 必须落在数据根 public/ 内
    if (!file.startsWith(path.join(DATA_ROOT, 'public') + path.sep) || !fs.existsSync(file) || !fs.statSync(file).isFile()) {
      return next()
    }
    const ext = rel.split('.').pop().toLowerCase()
    res.setHeader('Content-Type', MIME[ext] || 'application/octet-stream')
    res.setHeader('Content-Length', fs.statSync(file).size)
    fs.createReadStream(file).pipe(res)
  }
}

export default defineConfig({
  plugins: [
    {
      name: 'yukipan-dev-public',
      configureServer(server) {
        server.middlewares.use(publicFromDataRoot())
      },
    },
    vue(),
  ],
  server: {
    proxy: {
      '/api': { target: BACKEND, changeOrigin: true },
      '/public': { target: BACKEND, changeOrigin: true },
    },
  },
})
