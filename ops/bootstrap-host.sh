#!/usr/bin/env bash
# YukiPan 主机初始化：依赖、用户、目录、PostgreSQL、Redis、配置文件、nginx 与运维脚本。
#
# 交互模式（TTY）会逐项询问；非交互（如部署演练）使用默认值，
# 也可以用环境变量覆盖：
#   YUKIPAN_DOMAIN            站点域名（默认 pan.yeastar.xin）
#   YUKIPAN_CERT_EMAIL        证书通知邮箱（可留空，enable-https 时再给）
#   YUKIPAN_INSTALL_POSTGRES  true/false（默认 true；false 时需给 YUKIPAN_DATABASE_URL）
#   YUKIPAN_DATABASE_URL      外部 PostgreSQL 连接串（仅 YUKIPAN_INSTALL_POSTGRES=false）
#   YUKIPAN_INSTALL_REDIS     true/false（默认 true；false 时需给 YUKIPAN_REDIS_URL）
#   YUKIPAN_REDIS_URL         外部 Redis 连接串（仅 YUKIPAN_INSTALL_REDIS=false，须 redis:// 开头）
#   YUKIPAN_SKIP_PACKAGE_INSTALL=true  跳过 apt 安装，仅校验命令存在（演练用）

set -Eeuo pipefail
umask 077

readonly OPS_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly CONFIG_FILE="/etc/yukipan/config.toml"
readonly BOOTSTRAP_CONF="/etc/yukipan/bootstrap.conf"
readonly NGINX_AVAILABLE="/etc/nginx/sites-available/yukipan.conf"
readonly NGINX_ENABLED="/etc/nginx/sites-enabled/yukipan.conf"
readonly DEFAULT_DOMAIN="pan.yeastar.xin"

C_RED=$'\033[31m'; C_GREEN=$'\033[32m'; C_YELLOW=$'\033[33m'
C_BOLD=$'\033[1m'; C_NC=$'\033[0m'

info() { printf '%s[+]%s %s\n' "$C_GREEN" "$C_NC" "$*"; }
warn() { printf '%s[!]%s %s\n' "$C_YELLOW" "$C_NC" "$*" >&2; }
die() { printf '%s[x]%s %s\n' "$C_RED" "$C_NC" "$*" >&2; exit 1; }
title() { printf '\n%s%s%s\n' "$C_BOLD" "$*" "$C_NC"; }

[[ $EUID -eq 0 ]] || die "请以 root 运行"
[[ ! -e "$BOOTSTRAP_CONF" ]] \
    || die "$BOOTSTRAP_CONF 已存在；为避免覆盖密钥，已停止"

valid_domain() {
    [[ "$1" =~ ^([A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?\.)+[A-Za-z]{2,63}$ ]]
}

# prompt <说明> <默认值（可空）> -> 回答打印到 stdout
prompt() {
    local label="$1" default="$2" value
    if [[ -n "$default" ]]; then
        read -rp "  $label [$default]: " value
        printf '%s' "${value:-$default}"
    else
        read -rp "  $label（可留空）: " value
        printf '%s' "$value"
    fi
}

configure() {
    title "站点配置"
    if [[ ! -t 0 ]]; then
        DOMAIN="${YUKIPAN_DOMAIN:-$DEFAULT_DOMAIN}"
        CERT_EMAIL="${YUKIPAN_CERT_EMAIL:-}"
        INSTALL_POSTGRES="${YUKIPAN_INSTALL_POSTGRES:-true}"
        DATABASE_URL="${YUKIPAN_DATABASE_URL:-}"
        INSTALL_REDIS="${YUKIPAN_INSTALL_REDIS:-true}"
        REDIS_URL="${YUKIPAN_REDIS_URL:-}"
        info "非交互模式：域名 $DOMAIN，本机 PostgreSQL：$INSTALL_POSTGRES，本机 Redis：$INSTALL_REDIS"
    else
        DOMAIN="$(prompt '站点域名' "${YUKIPAN_DOMAIN:-$DEFAULT_DOMAIN}")"
        CERT_EMAIL="$(prompt '证书通知邮箱（enable-https 用，之后可补）' "${YUKIPAN_CERT_EMAIL:-}")"
        local answer
        read -rp "  在本机安装并初始化 PostgreSQL？[Y/n]: " answer
        if [[ "${answer,,}" == n ]]; then
            INSTALL_POSTGRES=false
            read -rp "  外部数据库 DATABASE_URL: " DATABASE_URL
        else
            INSTALL_POSTGRES=true
            DATABASE_URL=""
        fi
        read -rp "  在本机安装并初始化 Redis？[Y/n]: " answer
        if [[ "${answer,,}" == n ]]; then
            INSTALL_REDIS=false
            read -rp "  外部 Redis REDIS_URL: " REDIS_URL
        else
            INSTALL_REDIS=true
            REDIS_URL=""
        fi
    fi

    valid_domain "$DOMAIN" || die "域名格式无效：$DOMAIN"
    [[ -z "$CERT_EMAIL" || "$CERT_EMAIL" == *@*.* ]] \
        || die "证书通知邮箱格式无效：$CERT_EMAIL"
    if [[ "$INSTALL_POSTGRES" != true ]]; then
        [[ "$DATABASE_URL" == postgresql://* || "$DATABASE_URL" == postgres://* ]] \
            || die "YUKIPAN_INSTALL_POSTGRES=false 时需要合法的 DATABASE_URL"
    fi
    if [[ "$INSTALL_REDIS" != true ]]; then
        [[ "$REDIS_URL" == redis://* ]] \
            || die "YUKIPAN_INSTALL_REDIS=false 时需要 redis:// 开头的 REDIS_URL"
    fi
}

configure

install -d -m 755 /etc/yukipan
{
    printf 'DOMAIN=%q\n' "$DOMAIN"
    printf 'CERT_EMAIL=%q\n' "$CERT_EMAIL"
} > "$BOOTSTRAP_CONF"
chmod 600 "$BOOTSTRAP_CONF"

title "安装依赖"
if [[ "${YUKIPAN_SKIP_PACKAGE_INSTALL:-false}" == "true" ]]; then
    for command in nginx certbot psql pg_dump redis-cli curl openssl; do
        command -v "$command" >/dev/null \
            || die "演练镜像缺少依赖：$command"
    done
else
    packages=(nginx certbot postgresql-client redis-tools curl ca-certificates openssl)
    [[ "$INSTALL_POSTGRES" == true ]] && packages+=(postgresql)
    [[ "$INSTALL_REDIS" == true ]] && packages+=(redis-server)
    apt-get update
    DEBIAN_FRONTEND=noninteractive apt-get install -y "${packages[@]}"
fi
[[ "$INSTALL_POSTGRES" == true ]] && systemctl enable --now postgresql
[[ "$INSTALL_REDIS" == true ]] && systemctl enable --now redis-server

if ! id yukipan >/dev/null 2>&1; then
    useradd --system --home-dir /var/lib/yukipan \
        --shell /usr/sbin/nologin yukipan
fi

install -d -m 755 /var/www/yukipan/releases /var/www/yukipan/incoming
install -d -o yukipan -g yukipan -m 750 \
    /var/lib/yukipan /var/lib/yukipan/blobs /var/lib/yukipan/private \
    /var/lib/yukipan/public /var/lib/yukipan/public/images \
    /var/lib/yukipan/public/guest /var/lib/yukipan/tmp
# nginx 以 www-data 运行，借 yukipan 组直出 /public 与 /protected
usermod -aG yukipan www-data
install -d -m 700 /var/backups/yukipan
install -d -m 755 /usr/local/lib/yukipan/nginx
install -d -m 755 /etc/letsencrypt/renewal-hooks/deploy
install -d -o www-data -g www-data -m 755 /var/www/letsencrypt

if [[ "$INSTALL_POSTGRES" == true ]]; then
    title "初始化本机 PostgreSQL"
    readonly DATABASE_PASSWORD="$(openssl rand -hex 24)"
    if ! runuser -u postgres -- psql -tAc \
        "SELECT 1 FROM pg_roles WHERE rolname = 'yukipan'" | grep -qx '1'; then
        runuser -u postgres -- createuser --login yukipan
    fi
    runuser -u postgres -- psql --set=ON_ERROR_STOP=1 >/dev/null <<SQL
ALTER ROLE yukipan PASSWORD '$DATABASE_PASSWORD';
SQL
    if ! runuser -u postgres -- psql -tAc \
        "SELECT 1 FROM pg_database WHERE datname = 'yukipan'" | grep -qx '1'; then
        runuser -u postgres -- createdb --owner=yukipan yukipan
    fi
    DATABASE_URL="postgresql://yukipan:$DATABASE_PASSWORD@127.0.0.1:5432/yukipan"
    info "数据库 yukipan 已就绪"
fi

[[ "$INSTALL_REDIS" == true ]] && REDIS_URL="redis://127.0.0.1:6379"

# 后端进程以 yukipan 用户运行并自行读取该文件，故属主 yukipan 0600。
# 不能用 root:yukipan 0640: www-data 在 yukipan 组里 (见上面 usermod)，那样会读到库密码。
cat > "$CONFIG_FILE" <<EOF
# YukiPan 生产配置，由 bootstrap-host.sh 生成。
# 后端只认 YUKIPAN_CONFIG 环境变量指定的配置文件路径（systemd unit 已设置）。

listen = "127.0.0.1:8516"
data_root = "/var/lib/yukipan"
database_url = "$DATABASE_URL"
redis_url = "$REDIS_URL"

# 会话有效期 (小时), 过期需重新登录
session_ttl_hours = 168

# 配额与单文件上限, 单位字节; *_limit 是「引用配额」(同一内容挂多处按多处计);
# reserve 是真实磁盘余量红线。收新内容要同时过两道: 对应空间配额没超, 且盘上剩余 > reserve。
[quota]
# 私有网盘配额; 私有区无单文件上限, 大文件走这里
private_limit = 15032385536   # 14 GiB
# 图床配额
images_limit = 2147483648     # 2 GiB
# 访客网盘配额
guest_limit = 2147483648      # 2 GiB
# 余量红线; tmp/库/日志不进账本, 靠这道兜底
reserve = 2147483648          # 2 GiB
# 单文件大小限制: 按入口卡, 不按文件内容分类
guest_max_file = 52428800     # 50 MiB; 只卡 /api/guest/upload
images_max_file = 20971520    # 20 MiB; 只卡 /api/images/upload (另有扩展名白名单)
EOF
chmod 600 "$CONFIG_FILE"
chown yukipan:yukipan "$CONFIG_FILE"

install -m 644 "$OPS_DIR/systemd/yukipan-server.service" \
    /etc/systemd/system/yukipan-server.service
install -m 755 "$OPS_DIR/yukipan-deploy" /usr/local/sbin/yukipan-deploy
install -m 755 "$OPS_DIR/yukipan-backup" /usr/local/sbin/yukipan-backup
install -m 755 "$OPS_DIR/yukipan-restore" /usr/local/sbin/yukipan-restore
install -m 755 "$OPS_DIR/enable-https.sh" /usr/local/sbin/yukipan-enable-https
# 独立 hook 名，避免与同主机其他站点的 reload-nginx hook 互相覆盖
install -m 755 "$OPS_DIR/certbot-reload-nginx" \
    /etc/letsencrypt/renewal-hooks/deploy/yukipan-reload-nginx
# nginx 配置按域名实例化后存放，enable-https 时直接取用
sed "s/pan\.yeastar\.xin/$DOMAIN/g" "$OPS_DIR/nginx/yukipan.conf" \
    > /usr/local/lib/yukipan/nginx/yukipan.conf
chmod 644 /usr/local/lib/yukipan/nginx/yukipan.conf

if [[ -s "/etc/letsencrypt/live/$DOMAIN/fullchain.pem" ]]; then
    sed "s/pan\.yeastar\.xin/$DOMAIN/g" "$OPS_DIR/nginx/yukipan.conf" \
        > "$NGINX_AVAILABLE"
else
    sed "s/pan\.yeastar\.xin/$DOMAIN/g" "$OPS_DIR/nginx/yukipan-http-bootstrap.conf" \
        > "$NGINX_AVAILABLE"
fi
chmod 644 "$NGINX_AVAILABLE"
ln -sfn "$NGINX_AVAILABLE" "$NGINX_ENABLED"

systemctl daemon-reload
systemctl enable nginx
nginx -t
systemctl restart nginx

info "主机基础设施已建立，应用尚未启动"
cat <<EOF

下一步：
1. 将 release tar.gz 上传到 /var/www/yukipan/incoming/
2. 运行 sudo yukipan-deploy <release.tar.gz>
3. 创建第一个管理员用户（以 yukipan 身份运行, 交互式设置密码）：
   sudo -u yukipan /var/www/yukipan/current/bin/yukipan-server user add <用户名>
4. HTTP 验证正常后运行 sudo yukipan-enable-https${CERT_EMAIL:+ $CERT_EMAIL}。
EOF
