# 隔离部署演练

该工具在仓库的 `.rehearsal/` 中创建一次性 Ubuntu 24.04
`systemd-nspawn` 容器。容器拥有独立的根文件系统、PID、服务和网络命名空间，
不会安装或修改宿主机的软件包、nginx、PostgreSQL、Redis、systemd 配置或 `/etc`。

## 前置检查

宿主机只需已有 `systemd-nspawn`、`machinectl`、`curl`、`tar` 和 `sha256sum`。
脚本拒绝直接从 root 登录会话运行，必须由仓库所有者通过 `sudo` 启动。

发布目录中必须只有一组演练产物：

```text
.release-build/output/yukipan-<version>.tar.gz
.release-build/output/yukipan-<version>.tar.gz.sha256
```

## 运行

在仓库根目录执行：

```bash
sudo ./ops/rehearsal/run.sh
```

首次运行会从 Ubuntu 官方站点下载 rootfs、校验官方 SHA-256，并在基础镜像中安装
nginx、Certbot、PostgreSQL 和 Redis。依赖安装阶段只运行单个容器命令，不启动服务；
正式演练使用无外网的 private network namespace。

演练依次验证：

1. 干净 Ubuntu 主机初始化（bootstrap 以非交互模式运行），断言配置目录、
   数据目录树、系统用户、systemd unit、nginx 站点与 sbin 管理命令全部就位，
   PostgreSQL / Redis / nginx 均已启动；
2. 首次故障发布不会留下无效 `current`；
3. 首次正常部署、`/api/health` 健康检查（直连后端与经 nginx 各一次）；
4. 第二个健康 release 原子升级；
5. 故障 release 自动回滚；
6. PostgreSQL 与数据目录备份（`--with-data`）、修改和恢复，
   旧库与旧数据均按要求保留。

后端尚在编写中，演练的 HTTP 断言只覆盖探活口 `/api/health`；登录、上传等
接口级演练待前端与 API 定档后补充。

结果保存在：

```text
.rehearsal/runs/<UTC时间>-<PID>/results/
```

其中包含 `report.txt`、完整命令记录、systemd journal、服务状态、release 校验值和
备份清单。容器实例停止后不会残留运行中的服务或虚拟网卡。

## 清理

查看报告后执行：

```bash
./ops/rehearsal/cleanup.sh
```

清理脚本会再次验证路径、停止名称以 `yukipan-rehearsal-` 开头的遗留容器，并只删除
仓库内的 `.rehearsal/`。它不会访问项目外的数据。

演练不申请 Let’s Encrypt 证书，因为隔离容器不持有真实域名；HTTPS 配置和证书签发
应在最终服务器上单独完成。
