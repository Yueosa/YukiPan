<div align="center">

## YukiPan ☁️

</div>

**YukiPan** 是一个私人云端存储应用：私有文件柜、公开相册墙、访客临时收发口，
三个空间共享同一块按内容去重的磁盘 —— 同一份文件全站只存一份。

> 诞生原因：OpenList 功能太臃肿、形态单一，还出了一堆 Bug，
> 不如自己造一个完全贴合需求的。

---

## 整体架构

唯一真源是磁盘：文件按 SHA-256 内容寻址只存一份（`blobs/`），三个空间里看到的
都是指向它的 hardlink；「分享到图床/访客」只新建一条指向记录，不复制字节，
引用计数归零才真正删除。元数据与配额账本在 PostgreSQL，防风暴计数在 Redis。

* **私有存储：** 登录自用的文件资源管理器（上传/下载/预览/秒传，无单文件上限）
* **图床：** 公开相册墙（模仿 Google Photos），单图 20MB，稳定公开地址
* **访客空间：** 匿名上传拿链接，24 小时自动过期 + 按 IP 三道限流，不当公开垃圾桶

---

## 技术栈

**Backend (Rust)**
* Framework: `axum` + `tokio`（流式上传下载，内存钉死）
* Database: `sqlx` (PostgreSQL) + `redis`（限流计数）
* Auth: `argon2` + 会话 Cookie（HttpOnly）
* Storage: 内容寻址 blob + hardlink 引用计数（去重/秒传）

**Frontend (Vue)**
* `Vue 3` + `Vite` + `vue-router`，零 UI 框架，手写 CSS
* XHR 上传进度、分块 SHA-256 秒传尝试

**Infrastructure**
* PostgreSQL + Redis + Nginx + systemd（见 `ops/`）
* systemd-nspawn 隔离部署演练（见 `ops/rehearsal/`）

---

## 文档

[产品设计](./docs/yukipan.md) —— 三个空间的形态、去重与配额、全部接口、部署

[运维指南](./ops/README.md) —— 安装/发布/回滚/备份恢复/卸载

[隔离部署演练](./ops/rehearsal/README.md) —— 不碰宿主机的端到端验证

---

## 开发

```bash
# 后端 (workspace 在 yukipan/)
cargo test --workspace

# 便携 PostgreSQL / Redis 测试环境, 跑触真库测试
bash yukipan/dev/devdb.sh setup && bash yukipan/dev/devdb.sh start
bash yukipan/dev/devredis.sh setup && bash yukipan/dev/devredis.sh start
YUKIPAN_TEST_DB_URL=postgres://yukipan@127.0.0.1:54329/yukipan \
YUKIPAN_TEST_REDIS_URL=redis://127.0.0.1:56379 \
  cargo test --workspace -- --ignored

# 前端 (web/)
cd web && npm install && npm run dev
```

## 部署

服务器**不需要** Rust / Node 工具链：release 在开发机打包（后端二进制 + 前端静态页
+ SHA-256 清单），上传后由部署脚本原子切换。

```bash
# 开发机: 打包 (产物在 .release-build/output/)
./ops/build-release.sh <YYYYMMDDTHHMMSSZ-gitsha>

# 服务器 (clone 仓库拿 ops/ 脚本): 交互式初始化, 只需一次
sudo ./ops/bootstrap-host.sh        # 询问域名、证书邮箱、是否安装 PostgreSQL / Redis

# 把 tar.gz 与 .sha256 传到 /var/www/yukipan/incoming/ 后:
sudo yukipan-deploy /var/www/yukipan/incoming/yukipan-<version>.tar.gz
sudo -u yukipan /var/www/yukipan/current/bin/yukipan-server user add <用户名>
sudo yukipan-enable-https           # DNS 指向本机之后
```

发布前可用 nspawn 容器在隔离环境完整演练一遍（不碰宿主机）：

```bash
sudo ./ops/rehearsal/run.sh
```
