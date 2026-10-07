#!/usr/bin/env bash
# 为 YukiPan 申请 Let's Encrypt 证书并切换到 HTTPS 配置。
# 域名与邮箱默认取 bootstrap 写入的 /etc/yukipan/bootstrap.conf，可用参数覆盖。

set -Eeuo pipefail

readonly BOOTSTRAP_CONF="/etc/yukipan/bootstrap.conf"
readonly AVAILABLE="/etc/nginx/sites-available/yukipan.conf"
readonly HTTPS_CONFIG="/usr/local/lib/yukipan/nginx/yukipan.conf"

[[ $EUID -eq 0 ]] || { echo "请以 root 运行" >&2; exit 1; }

DOMAIN="pan.yeastar.xin"
CERT_EMAIL=""
if [[ -f "$BOOTSTRAP_CONF" ]]; then
    # 由 bootstrap-host.sh 生成，仅含 shell 转义的 DOMAIN/CERT_EMAIL
    # shellcheck disable=SC1090
    source "$BOOTSTRAP_CONF"
fi

EMAIL="${1:-$CERT_EMAIL}"
[[ "$EMAIL" == *@*.* ]] \
    || { echo "用法：yukipan-enable-https <证书通知邮箱>（或在 bootstrap 时配置）" >&2; exit 2; }

certbot certonly --webroot --webroot-path /var/www/letsencrypt \
    --cert-name "$DOMAIN" \
    --domain "$DOMAIN" \
    --email "$EMAIL" --agree-tos --non-interactive

install -m 644 "$HTTPS_CONFIG" "$AVAILABLE"
nginx -t
systemctl reload nginx
printf 'HTTPS 已启用：https://%s\n' "$DOMAIN"
