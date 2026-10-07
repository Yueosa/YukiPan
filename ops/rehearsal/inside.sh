#!/usr/bin/env bash

set -Eeuo pipefail
umask 027

readonly INPUT="/root/rehearsal-input"
readonly RESULTS="/root/rehearsal-results"
readonly WORK="/root/rehearsal-work"
mkdir -p "$RESULTS" "$WORK"
exec > >(tee -a "$RESULTS/transcript.log") 2>&1

collect_diagnostics() {
    local status="$1"
    systemctl --no-pager --full status \
        postgresql.service redis-server.service nginx.service \
        yukipan-server.service >"$RESULTS/services.txt" 2>&1 || true
    journalctl --no-pager -b \
        -u postgresql.service -u redis-server.service \
        -u nginx.service -u yukipan-server.service \
        >"$RESULTS/journal.txt" 2>&1 || true
    {
        printf 'exit_status=%s\n' "$status"
        printf 'container_os='
        . /etc/os-release
        printf '%s %s\n' "$NAME" "$VERSION_ID"
        printf 'kernel=%s\n' "$(uname -r)"
    } >"$RESULTS/environment.txt"
}
trap 'status=$?; collect_diagnostics "$status"' EXIT

fail() {
    echo "演练断言失败：$*" >&2
    exit 1
}

wait_ready() {
    local ready=false
    for _ in {1..40}; do
        if curl -fsS http://127.0.0.1:8516/api/health >/dev/null; then
            ready=true
            break
        fi
        sleep 1
    done
    [[ "$ready" == true ]] || fail "应用未进入 ready 状态"
}

make_release() {
    local source="$1" version="$2" mode="$3" destination="$4"
    local stage="$WORK/stage-$version"
    rm -rf -- "$stage"
    mkdir -p "$stage"
    tar -xzf "$source" -C "$stage"
    printf '%s\n' "$version" >"$stage/RELEASE"
    if [[ "$mode" == fail ]]; then
        cat >"$stage/bin/yukipan-server" <<'EOF'
#!/bin/sh
echo "intentional rehearsal failure" >&2
exit 42
EOF
        chmod 755 "$stage/bin/yukipan-server"
    fi
    local manifest_dirs=(bin)
    [[ ! -d "$stage/web" ]] || manifest_dirs+=(web)
    (
        cd "$stage"
        find "${manifest_dirs[@]}" -type f -print0 \
            | LC_ALL=C sort -z \
            | xargs -0 sha256sum >MANIFEST.sha256
        sha256sum RELEASE >>MANIFEST.sha256
    )
    tar -C "$stage" -czf "$destination" .
    (
        cd "$(dirname "$destination")"
        sha256sum "$(basename "$destination")" \
            >"$(basename "$destination").sha256"
    )
}

mapfile -t source_archives < <(
    printf '%s\n' "$INPUT"/yukipan-*.tar.gz \
        | while IFS= read -r archive; do [[ -f "$archive" ]] && printf '%s\n' "$archive"; done
)
((${#source_archives[@]} == 1)) || fail "输入 release 数量不是 1"
readonly SOURCE_ARCHIVE="${source_archives[0]}"
(
    cd "$INPUT"
    sha256sum -c "$(basename "$SOURCE_ARCHIVE").sha256"
)
readonly SOURCE_VERSION="$(tar -xOf "$SOURCE_ARCHIVE" ./RELEASE | tr -d '\r\n')"
readonly SOURCE_SHA="${SOURCE_VERSION#*-}"
[[ "$SOURCE_SHA" =~ ^[a-f0-9]{7,40}$ ]] || fail "源 release 版本无效"

readonly FIRST_FAIL_VERSION="20990101T000001Z-$SOURCE_SHA"
readonly HEALTHY_VERSION="20990101T000002Z-$SOURCE_SHA"
readonly ROLLBACK_FAIL_VERSION="20990101T000003Z-$SOURCE_SHA"
readonly FIRST_FAIL="$WORK/yukipan-$FIRST_FAIL_VERSION.tar.gz"
readonly HEALTHY="$WORK/yukipan-$HEALTHY_VERSION.tar.gz"
readonly ROLLBACK_FAIL="$WORK/yukipan-$ROLLBACK_FAIL_VERSION.tar.gz"
make_release "$SOURCE_ARCHIVE" "$FIRST_FAIL_VERSION" fail "$FIRST_FAIL"
make_release "$SOURCE_ARCHIVE" "$HEALTHY_VERSION" healthy "$HEALTHY"
make_release "$SOURCE_ARCHIVE" "$ROLLBACK_FAIL_VERSION" fail "$ROLLBACK_FAIL"

echo "== 初始化干净 Ubuntu 主机 =="
YUKIPAN_SKIP_PACKAGE_INSTALL=true "$INPUT/ops/bootstrap-host.sh" </dev/null
[[ -f /etc/yukipan/bootstrap.conf ]] || fail "缺少 bootstrap.conf"
[[ -f /etc/yukipan/config.toml ]] || fail "缺少 config.toml"
id yukipan >/dev/null || fail "缺少系统用户 yukipan"
for dir in blobs private public/images public/guest tmp; do
    [[ -d "/var/lib/yukipan/$dir" ]] || fail "缺少数据目录：$dir"
done
[[ -f /etc/systemd/system/yukipan-server.service ]] || fail "缺少 systemd unit"
[[ -x /usr/local/sbin/yukipan-deploy ]] || fail "缺少 yukipan-deploy"
[[ -x /usr/local/sbin/yukipan-backup ]] || fail "缺少 yukipan-backup"
[[ -x /usr/local/sbin/yukipan-restore ]] || fail "缺少 yukipan-restore"
[[ -f /etc/nginx/sites-enabled/yukipan.conf ]] || fail "缺少 nginx 站点"
[[ -f /usr/local/lib/yukipan/nginx/yukipan.conf ]] || fail "缺少 HTTPS 模板存档"
grep -q '^database_url = "postgresql://yukipan:' /etc/yukipan/config.toml \
    || fail "config.toml 未写入本机 database_url"
grep -q '^redis_url = "redis://127.0.0.1:6379"' /etc/yukipan/config.toml \
    || fail "config.toml 未写入本机 redis_url"
systemctl is-active --quiet postgresql.service || fail "PostgreSQL 未启动"
systemctl is-active --quiet redis-server.service || fail "Redis 未启动"
systemctl is-active --quiet nginx.service || fail "nginx 未启动"

install_release() {
    local archive="$1"
    install -m 644 "$archive" "$archive.sha256" /var/www/yukipan/incoming/
    yukipan-deploy "/var/www/yukipan/incoming/$(basename "$archive")"
}

echo "== 验证首次发布失败不会留下 current =="
if install_release "$FIRST_FAIL"; then
    fail "故障 release 意外部署成功"
fi
[[ ! -e /var/www/yukipan/current ]] || fail "首次失败后仍存在 current"
systemctl is-active --quiet yukipan-server.service \
    && fail "首次失败后服务仍处于 active"

echo "== 首次部署真实 release =="
install_release "$SOURCE_ARCHIVE"
wait_ready
curl -fsS -H 'Host: pan.yeastar.xin' \
    http://127.0.0.1/api/health >/dev/null
[[ "$(basename "$(readlink -f /var/www/yukipan/current)")" == "$SOURCE_VERSION" ]] \
    || fail "current 未指向源 release"

echo "== 验证健康原子升级 =="
install_release "$HEALTHY"
wait_ready
[[ "$(basename "$(readlink -f /var/www/yukipan/current)")" == "$HEALTHY_VERSION" ]] \
    || fail "健康升级未切换 current"

echo "== 验证升级失败自动回滚 =="
if install_release "$ROLLBACK_FAIL"; then
    fail "故障升级意外成功"
fi
wait_ready
[[ "$(basename "$(readlink -f /var/www/yukipan/current)")" == "$HEALTHY_VERSION" ]] \
    || fail "故障升级未回滚到健康 release"

echo "== 验证数据库与数据目录备份恢复 =="
DATABASE_URL="$(sed -n \
    's/^database_url[[:space:]]*=[[:space:]]*"\(.*\)"[[:space:]]*$/\1/p' \
    /etc/yukipan/config.toml | head -n 1)"
[[ -n "$DATABASE_URL" ]] || fail "config.toml 中缺少 database_url"
readonly DATABASE_URL

psql "$DATABASE_URL" --set=ON_ERROR_STOP=1 >/dev/null <<'SQL'
CREATE TABLE rehearsal_marker (id integer PRIMARY KEY, value text NOT NULL);
INSERT INTO rehearsal_marker VALUES (1, 'before-backup');
SQL
printf 'before-backup\n' >/var/lib/yukipan/public/guest/rehearsal-marker.txt
chown yukipan:yukipan /var/lib/yukipan/public/guest/rehearsal-marker.txt
yukipan-backup --with-data
readonly BACKUP_ID="$(basename "$(ls -1dt /var/backups/yukipan/* | sed -n '1p')")"
[[ "$BACKUP_ID" =~ ^[0-9]{8}T[0-9]{6}Z-[0-9]{9}$ ]] \
    || fail "备份 ID 格式无效：$BACKUP_ID"
[[ -f "/var/backups/yukipan/$BACKUP_ID/database.dump" ]] \
    || fail "备份缺少 database.dump"
[[ -f "/var/backups/yukipan/$BACKUP_ID/data.tar.gz" ]] \
    || fail "备份缺少 data.tar.gz"
(
    cd "/var/backups/yukipan/$BACKUP_ID"
    sha256sum --check MANIFEST.sha256 >/dev/null
) || fail "备份 MANIFEST 校验失败"

psql "$DATABASE_URL" --set=ON_ERROR_STOP=1 \
    -c "UPDATE rehearsal_marker SET value = 'after-backup' WHERE id = 1" >/dev/null
printf 'after-backup\n' >/var/lib/yukipan/public/guest/rehearsal-marker.txt
yukipan-restore "$BACKUP_ID" --confirm-restore --with-data
wait_ready

readonly DB_VALUE="$(
    psql "$DATABASE_URL" -Atc 'SELECT value FROM rehearsal_marker WHERE id = 1'
)"
readonly DATA_VALUE="$(tr -d '\r\n' </var/lib/yukipan/public/guest/rehearsal-marker.txt)"
[[ "$DB_VALUE" == before-backup ]] || fail "数据库未恢复到备份值"
[[ "$DATA_VALUE" == before-backup ]] || fail "数据目录未恢复到备份值"
readonly SAFE_BACKUP_ID="${BACKUP_ID//[^0-9A-Za-z]/_}"
runuser -u postgres -- psql -Atc \
    "SELECT 1 FROM pg_database WHERE datname = 'yukipan_before_$SAFE_BACKUP_ID'" \
    | grep -qx 1 || fail "恢复后未保留旧数据库"
[[ -d "/var/lib/yukipan/data-before-$BACKUP_ID" ]] \
    || fail "恢复后未保留旧数据"

sha256sum "$SOURCE_ARCHIVE" >"$RESULTS/release.sha256"
cp -a "/var/backups/yukipan/$BACKUP_ID/MANIFEST.sha256" \
    "$RESULTS/backup-manifest.sha256"
cat >"$RESULTS/report.txt" <<EOF
result=passed
source_release=$SOURCE_VERSION
healthy_release=$HEALTHY_VERSION
rollback_target=$HEALTHY_VERSION
backup_id=$BACKUP_ID
database_restore=$DB_VALUE
data_restore=$DATA_VALUE
EOF
echo "隔离演练全部通过"
