// 增量 SHA-256: 秒传需要对齐服务端的整文件 SHA-256, 但 Web Crypto 不支持增量 digest,
// 大文件又不能整读内存 — 这里手写标准算法, 分块喂入。
// 正确性由 dev/sha256 对照测试 (Node crypto) 与秒传联调保证。

const K = new Uint32Array([
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
  0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
  0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
  0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
  0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
  0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
  0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
  0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
])

function rotr(x, n) {
  return (x >>> n) | (x << (32 - n))
}

export class Sha256 {
  constructor() {
    this.h = new Uint32Array([
      0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ])
    this.block = new Uint8Array(64)
    this.blockLen = 0
    this.total = 0 // 字节数 (文件 < 2^53, number 精确)
    this.w = new Uint32Array(64)
  }

  update(data) {
    let offset = 0
    this.total += data.length
    while (offset < data.length) {
      const take = Math.min(64 - this.blockLen, data.length - offset)
      this.block.set(data.subarray(offset, offset + take), this.blockLen)
      this.blockLen += take
      offset += take
      if (this.blockLen === 64) {
        this.compress(this.block)
        this.blockLen = 0
      }
    }
  }

  compress(b) {
    const w = this.w
    for (let i = 0; i < 16; i++) {
      w[i] = (b[i * 4] << 24) | (b[i * 4 + 1] << 16) | (b[i * 4 + 2] << 8) | b[i * 4 + 3]
    }
    for (let i = 16; i < 64; i++) {
      const s0 = rotr(w[i - 15], 7) ^ rotr(w[i - 15], 18) ^ (w[i - 15] >>> 3)
      const s1 = rotr(w[i - 2], 17) ^ rotr(w[i - 2], 19) ^ (w[i - 2] >>> 10)
      w[i] = (w[i - 16] + s0 + w[i - 7] + s1) | 0
    }
    let [a, b0, c, d, e, f, g, h] = this.h
    for (let i = 0; i < 64; i++) {
      const S1 = rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25)
      const ch = (e & f) ^ (~e & g)
      const t1 = (h + S1 + ch + K[i] + w[i]) | 0
      const S0 = rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22)
      const maj = (a & b0) ^ (a & c) ^ (b0 & c)
      const t2 = (S0 + maj) | 0
      h = g; g = f; f = e; e = (d + t1) | 0
      d = c; c = b0; b0 = a; a = (t1 + t2) | 0
    }
    const hh = this.h
    hh[0] = (hh[0] + a) | 0
    hh[1] = (hh[1] + b0) | 0
    hh[2] = (hh[2] + c) | 0
    hh[3] = (hh[3] + d) | 0
    hh[4] = (hh[4] + e) | 0
    hh[5] = (hh[5] + f) | 0
    hh[6] = (hh[6] + g) | 0
    hh[7] = (hh[7] + h) | 0
  }

  digestHex() {
    // 填充: 1 bit + 0 填充 + 64 位大端位长
    const bitLenHi = Math.floor(this.total / 0x20000000)
    const bitLenLo = (this.total * 8) >>> 0
    const tail = new Uint8Array(this.blockLen < 56 ? 64 : 128)
    tail.set(this.block.subarray(0, this.blockLen))
    tail[this.blockLen] = 0x80
    const dv = new DataView(tail.buffer)
    dv.setUint32(tail.length - 8, bitLenHi)
    dv.setUint32(tail.length - 4, bitLenLo)
    this.compress(tail.subarray(0, 64))
    if (tail.length === 128) this.compress(tail.subarray(64, 128))
    return Array.from(this.h, (x) => (x >>> 0).toString(16).padStart(8, '0')).join('')
  }
}

// 分块读文件算 SHA-256, 不整读内存。onProgress(doneBytes, totalBytes); shouldCancel() 返回 true 时中止。
export async function hashFile(file, { onProgress, shouldCancel, chunkSize = 4 * 1024 * 1024 } = {}) {
  const h = new Sha256()
  let offset = 0
  while (offset < file.size) {
    if (shouldCancel && shouldCancel()) throw new Error('已取消')
    const end = Math.min(offset + chunkSize, file.size)
    const buf = new Uint8Array(await file.slice(offset, end).arrayBuffer())
    h.update(buf)
    offset = end
    if (onProgress) onProgress(offset, file.size)
    // 让出主线程, 进度条与界面能渲染
    await new Promise((r) => setTimeout(r, 0))
  }
  return h.digestHex()
}
