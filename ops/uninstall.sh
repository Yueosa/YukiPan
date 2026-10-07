#!/usr/bin/env bash
# YukiPan 卸载：停服务、撤 nginx 站点、卸载前先自动备份一次。
# 默认保留数据与配置：/var/lib/yukipan、/var/backups/yukipan、/etc/yukipan、
# 系统用户 yukipan；--purge 才全删，且必须输入站点域名确认（双重确认）。
# 用法：sudo ./ops/uninstall.sh [--purge]

set -Eeuo pipefail

readonly UNIT="yukipan-server.service"
readonly UNIT_FILE="/etc/systemd/system/yukipan-server.service"
readonly NGINX_AVAILABLE="/etc/nginx/sites-available/yukipan.conf"
readonly NGINX_ENABLED="/etc/nginx/sites-enabled/yukipan.conf"
readonly CERTBOT_HOOK="/etc/letsencrypt/renewal-hooks/deploy/yukipan-reload-nginx"
readonly BOOTSTRAP_CONF="/etc/yukipan/bootstrap.conf"
readonly ETC_DIR="/etc/yukipan"
readonly WWW_ROOT="/var/www/yukipan"
readonly DATA_ROOT="/var/lib/yukipan"
readonly BACKUP_ROOT="/var/backups/yukipan"
readonly LIB_DIR="/usr/local/lib/yukipan"
readonly SBIN_COMMANDS=(yukipan-deploy yukipan-backup yukipan-restore yukipan-enable-https)

C_RED=$'\033[31m'; C_GREEN=$'\033[32m'; C_YELLOW=$'\033[33m'
C_BOLD=$'\033[1m'; C_NC=$'\033[0m'

info() { printf '%s[+]%s %s\n' "$C_GREEN" "$C_NC" "$*"; }
warn() { printf '%s[!]%s %s\n' "$C_YELLOW" "$C_NC" "$*" >&2; }
die() { printf '%s[x]%s %s\n' "$C_RED" "$C_NC" "$*" >&2; exit 1; }
title() { printf '\n%s%s%s\n' "$C_BOLD" "$*" "$C_NC"; }

[[ $EUID -eq 0 ]] || die "请以 root 运行"

PURGE=false
case "${1:-}" in
    "") ;;
    --purge) PURGE=true ;;
    *) die "用法：uninstall.sh [--purge]" ;;
esac
readonly PURGE

title "停止并禁用服务"
if systemctl list-unit-files "$UNIT" >/dev/null 2>&1; then
    systemctl disable --now "$UNIT" 2>/dev/null || true
fi

title "撤除 nginx 站点"
rm -f -- "$NGINX_ENABLED" "$NGINX_AVAILABLE"
if command -v nginx >/dev/null; then
    nginx -t && systemctl reload nginx
fi

title "卸载前备份"
if [[ -x /usr/local/sbin/yukipan-backup && -r "$ETC_DIR/config.toml" ]]; then
    /usr/local/sbin/yukipan-backup \
        || die "自动备份失败，为安全起见已停止卸载；请排查后重试"
else
    warn "备份命令或配置不可用，跳过卸载前备份"
fi

title "移除程序与集成"
for command in "${SBIN_COMMANDS[@]}"; do
    rm -f -- "/usr/local/sbin/$command"
done
rm -f -- "$UNIT_FILE" "$CERTBOT_HOOK"
rm -rf -- "$LIB_DIR"
rm -rf -- "$WWW_ROOT"
systemctl daemon-reload

if [[ "$PURGE" != true ]]; then
    info "YukiPan 已卸载。以下内容被保留："
    cat <<EOF
  数据目录   $DATA_ROOT
  备份目录   $BACKUP_ROOT
  配置目录   $ETC_DIR
  系统用户   yukipan
  PostgreSQL 数据库 yukipan 与角色 yukipan（如为本机建库）
如需彻底删除，请运行：sudo $0 --purge
EOF
    exit 0
fi

title "彻底删除（--purge）"
[[ -t 0 ]] || die "--purge 需要交互终端确认"

DOMAIN=""
if [[ -f "$BOOTSTRAP_CONF" ]]; then
    # 由 bootstrap-host.sh 生成，仅含 shell 转义的 DOMAIN/CERT_EMAIL
    # shellcheck disable=SC1090
    source "$BOOTSTRAP_CONF"
fi

warn "将永久删除：$DATA_ROOT、$BACKUP_ROOT、$ETC_DIR（含刚做的卸载前备份以外的全部历史备份）"
read -rp "  第一次确认：继续彻底删除？[y/N]: " answer
[[ "${answer,,}" == y ]] || die "已取消"

if [[ -n "$DOMAIN" ]]; then
    read -rp "  第二次确认：请输入站点域名 $DOMAIN: " answer
    [[ "$answer" == "$DOMAIN" ]] || die "域名不匹配，已取消"
else
    read -rp "  第二次确认：请输入 yukipan: " answer
    [[ "$answer" == yukipan ]] || die "确认输入不匹配，已取消"
fi

rm -rf -- "$DATA_ROOT" "$BACKUP_ROOT" "$ETC_DIR"
info "数据、备份与配置已删除"

if command -v runuser >/dev/null && id postgres >/dev/null 2>&1; then
    read -rp "  同时删除 PostgreSQL 数据库 yukipan 与角色 yukipan？[y/N]: " answer
    if [[ "${answer,,}" == y ]]; then
        runuser -u postgres -- dropdb --if-exists yukipan
        runuser -u postgres -- dropuser --if-exists yukipan
        info "数据库与角色已删除"
    fi
fi

if id yukipan >/dev/null 2>&1; then
    read -rp "  同时删除系统用户 yukipan？[y/N]: " answer
    if [[ "${answer,,}" == y ]]; then
        userdel yukipan
        info "系统用户已删除"
    fi
fi

info "YukiPan 已彻底卸载"
