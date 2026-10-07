# Operations

## 生产目录

- nginx 终止 TLS 并直出 `/public/images/`、`/public/guest/` 静态内容；
- systemd 管理后端服务（单个 unit，无独立 worker）；
- `/var/www/yukipan/releases/<release>` 保存不可变发布产物；
- `/var/www/yukipan/current` 原子指向当前版本；
- `/var/lib/yukipan` 是数据根（`blobs/` 内容寻址存储、`private/` 私有区、
  `public/images` 图床、`public/guest` 访客空间、`tmp/` 上传暂存）；
- `/var/backups/yukipan` 保存数据库与配置备份（700）；
- `/etc/yukipan/config.toml` 保存运行配置（600 yukipan:yukipan，含数据库口令，
  后端进程以自己的身份读取；www-data 无权读），
  `/etc/yukipan/bootstrap.conf` 保存域名与证书邮箱默认值。

release 包含后端二进制（`bin/yukipan-server`）、可选的前端静态页（`web/`）和
SHA-256 清单，不包含配置、数据、数据库或历史备份。

## nginx 层

`ops/nginx/yukipan.conf` 安装到 `sites-available`（即 http 上下文），除反向代理外还负责：

- 安全响应头：`Strict-Transport-Security`（HSTS）、`nosniff`、`X-Frame-Options`、
  `Referrer-Policy`、`Permissions-Policy` 与 CSP（前端未定档，先按无内联脚本的
  SPA 收紧 `script-src 'self'`）；
- `/public/images/`、`/public/guest/` 以 `alias` 直出数据根下的公开目录
  （区目录内是 blob 的 hardlink，文件流不经过后端内存）；访客下载另加每 IP
  并发 4（`limit_conn_zone yukipan_guest_conn`）与单连接 2 MB/s 限速防风暴；
- `/protected/` 标记 `internal`，仅供后端鉴权后 X-Accel-Redirect 内部跳转；
- `/api/` 反代 `127.0.0.1:8516` 并关闭请求缓冲；server 级
  `client_max_body_size 16g`（私有区无单文件上限），
  `location = /api/guest/upload` 单独卡 50m。

## 构建

版本号使用 UTC 时间和 Git commit：

```bash
VERSION="$(date -u +%Y%m%dT%H%M%SZ)-$(git rev-parse --short=12 HEAD)"
./ops/build-release.sh "$VERSION"
```

构建在 `yukipan/` workspace 下执行 `cargo build --release`，默认复用已有的
`yukipan/target` 与系统 Cargo 缓存；crate 二进制名 `yukipan` 在打包时重命名为
`bin/yukipan-server` 以对齐部署约定。前端未定档：存在 `web/dist`
（可用 `YUKIPAN_WEB_DIST` 覆盖路径）时一并打进 `web/`，否则只发后端并告警。
产物输出到 `.release-build/output/yukipan-<version>.tar.gz` 与同名 `.sha256`。

## 首次主机初始化

将当前 `ops/` 目录复制到 Ubuntu 服务器，并把 release tarball 及同名
`.sha256` 一起上传后运行：

```bash
sudo ./ops/bootstrap-host.sh
sudo yukipan-deploy /var/www/yukipan/incoming/yukipan-<version>.tar.gz
sudo yukipan-enable-https
```

初始化脚本在终端下逐项询问：站点域名（默认 `pan.yeastar.xin`）、证书通知邮箱
（可留空）、是否在本机安装 PostgreSQL（选否则需提供外部 `DATABASE_URL`）、
是否在本机安装 Redis（选否则需提供 `redis://` 开头的 `REDIS_URL`）。
非交互环境（如部署演练）直接使用默认值，也可用 `YUKIPAN_DOMAIN`、
`YUKIPAN_CERT_EMAIL`、`YUKIPAN_INSTALL_POSTGRES`、`YUKIPAN_DATABASE_URL`、
`YUKIPAN_INSTALL_REDIS`、`YUKIPAN_REDIS_URL` 环境变量覆盖。域名与邮箱写入
`/etc/yukipan/bootstrap.conf`，nginx 配置按域名实例化，`yukipan-enable-https`
会从该文件读取默认值，不再要求必须传邮箱参数。

随后脚本安装 nginx、Certbot、PostgreSQL、Redis 客户端（按需装服务端），创建无登录
shell 的 `yukipan` 用户、数据库和随机数据库密码。
若 `/etc/yukipan/bootstrap.conf` 已经存在，脚本会拒绝覆盖。

首次部署 release 之后，创建第一个管理员用户（交互式输入两遍密码）：

```bash
sudo /var/www/yukipan/current/bin/yukipan-server user add <用户名>
```

## 发布与回滚

`yukipan-deploy` 串行执行：

1. 拒绝危险 tar 路径并验证外部与内部 SHA-256；
2. 创建数据库与配置备份（不带 `--with-data`）；
3. 原子替换 `current` 软链接；
4. 重启服务并 30 秒轮询 `/api/health`；
5. 健康检查失败时打印 journal 尾部并恢复上一条软链接。

YukiPan 暂无独立 migration 步骤（后端在写，schema 由应用自身管理），deploy 脚本
中留有占位注释；将来引入迁移工具时加在备份之后、切换之前，与 YukiLog 位置一致。

release 不会被自动删除。应用回滚不会自动回滚数据库。

## 备份与恢复

```bash
sudo yukipan-backup                 # 默认：数据库 + 配置，不含 /var/lib/yukipan 数据
sudo yukipan-backup --with-data     # 连数据目录一起打 tar（20G 量级，慎用）
sudo yukipan-restore 20261007T150000Z-123456789 --confirm-restore [--with-data]
```

备份目录包含 PostgreSQL custom dump、配置副本、release 路径和校验清单；
工具不会自动清理旧备份。数据备份的 tar 保留 blobs/private/public 之间的
hardlink 关系；`tmp/` 暂存不进备份。

恢复会先把 dump 导入临时数据库，成功后才停止服务并原子交换数据库名。旧数据库
（以及 `--with-data` 时的旧数据目录）都会保留，不会直接删除。恢复属于破坏性操作，
因此必须提供精确确认参数。

## 卸载

```bash
sudo ./ops/uninstall.sh            # 停服务、撤站点、先自动备份一次；保留数据/备份/配置/用户
sudo ./ops/uninstall.sh --purge    # 全删，需输入站点域名二次确认
```

默认模式明确打印保留内容；`--purge` 删除数据、备份、配置，并分别询问是否删除
PostgreSQL 数据库/角色与系统用户。

## 隔离部署演练

`ops/rehearsal/` 提供基于 systemd-nspawn 的端到端演练（干净 Ubuntu 24.04 容器里
跑 bootstrap → deploy → 升级 → 回滚 → 备份恢复全流程），用法见
`ops/rehearsal/README.md`。nspawn rehearsal 演练体系即本目录的既有能力；
更高保真的演练项（HTTPS 签发、接口级用例）可作为后续项，本次未做。

## 目录/文件清单

| 路径 | 用途 | 属主/权限 |
| --- | --- | --- |
| `/etc/yukipan/config.toml` | 后端运行配置（含密钥） | yukipan:yukipan 600 |
| `/etc/yukipan/bootstrap.conf` | 域名/邮箱默认值 | root:root 600 |
| `/etc/systemd/system/yukipan-server.service` | systemd unit | root:root 644 |
| `/usr/local/sbin/yukipan-{deploy,backup,restore}` | 运维命令 | root:root 755 |
| `/usr/local/sbin/yukipan-enable-https` | HTTPS 切换 | root:root 755 |
| `/etc/letsencrypt/renewal-hooks/deploy/yukipan-reload-nginx` | 续期 hook | root:root 755 |
| `/usr/local/lib/yukipan/nginx/yukipan.conf` | HTTPS 模板存档（按域名实例化） | root:root 644 |
| `/etc/nginx/sites-{available,enabled}/yukipan.conf` | 当前生效站点 | root:root 644 |
| `/var/www/yukipan/releases/<version>` | 不可变 release | root:root a-w |
| `/var/www/yukipan/current` | 当前版本软链 | - |
| `/var/www/yukipan/incoming/` | 上传暂存 | root:root 755 |
| `/var/lib/yukipan/{blobs,private,public,tmp}` | 数据根 | yukipan:yukipan 750 |
| `/var/backups/yukipan/<id>` | 备份 | root:root 700 |
| `/var/www/letsencrypt` | ACME webroot | www-data 755 |

## 常用命令

```bash
systemctl status yukipan-server.service
systemctl restart yukipan-server.service
journalctl -u yukipan-server.service -f
journalctl -u yukipan-server.service --since today
nginx -t && systemctl reload nginx
```
