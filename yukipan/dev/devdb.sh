#!/usr/bin/env bash
# YukiPan 开发用 PostgreSQL: 便携二进制 (zonky embedded-postgres), 不进系统。
# 一切都在 yukipan/target/devdb/ 下 (target 已 gitignore, cargo clean 也会清掉)。
#
#   dev/devdb.sh setup     下载二进制并初始化数据目录 (幂等)
#   dev/devdb.sh start     启动, 监听 127.0.0.1:54329, trust 认证, 库名 yukipan
#   dev/devdb.sh stop      停止
#   dev/devdb.sh destroy   停止并删除整个 devdb 目录 (完全移除)
set -euo pipefail

DEVDB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/target/devdb"
PG_DIR="$DEVDB_DIR/pg"
DATA_DIR="$DEVDB_DIR/data"
SOCK_DIR="$DEVDB_DIR/sock"
LOG="$DEVDB_DIR/postgres.log"
PORT=54329
PG_VERSION=17.10.0
JAR_URL="https://repo1.maven.org/maven2/io/zonky/test/postgres/embedded-postgres-binaries-linux-amd64/$PG_VERSION/embedded-postgres-binaries-linux-amd64-$PG_VERSION.jar"

running() {
  [ -f "$DATA_DIR/postmaster.pid" ]
}

case "${1:-}" in
  setup)
    if [ ! -x "$PG_DIR/bin/initdb" ]; then
      mkdir -p "$DEVDB_DIR"
      echo "下载 PostgreSQL $PG_VERSION 便携二进制..."
      curl -fL "$JAR_URL" -o "$DEVDB_DIR/pg-binaries.jar"
      JARDIR="$DEVDB_DIR/.jar"
      rm -rf "$JARDIR"
      mkdir -p "$JARDIR"
      bsdtar -xf "$DEVDB_DIR/pg-binaries.jar" -C "$JARDIR"
      TXZ="$(find "$JARDIR" -name '*.txz' | head -n1)"
      [ -n "$TXZ" ] || { echo "jar 里没找到 .txz"; exit 1; }
      rm -rf "$PG_DIR"
      mkdir -p "$PG_DIR"
      tar -xJf "$TXZ" -C "$PG_DIR"
      # 包内可能多套一层目录, 归一化成 $PG_DIR/bin/...
      if [ ! -x "$PG_DIR/bin/initdb" ]; then
        INNER="$(find "$PG_DIR" -mindepth 1 -maxdepth 1 -type d | head -n1)"
        mv "$INNER"/* "$PG_DIR"/ && rmdir "$INNER"
      fi
      rm -rf "$JARDIR" "$DEVDB_DIR/pg-binaries.jar"
    fi
    if [ ! -d "$DATA_DIR" ]; then
      "$PG_DIR/bin/initdb" -D "$DATA_DIR" -U yukipan -A trust -E UTF8 --locale=C
    fi
    echo "setup 完成。启动: $0 start"
    ;;
  start)
    running && { echo "已在运行 (pid $(head -1 "$DATA_DIR/postmaster.pid"))"; exit 0; }
    mkdir -p "$SOCK_DIR"
    "$PG_DIR/bin/pg_ctl" -D "$DATA_DIR" -l "$LOG" -w \
      -o "-p $PORT -k $SOCK_DIR -c listen_addresses=127.0.0.1" \
      start
    # 便携包只有服务端二进制, 客户端用系统 psql (postgresql-libs)
    if ! psql -h 127.0.0.1 -p "$PORT" -U yukipan -d postgres -lqtA | grep -qx yukipan; then
      psql -h 127.0.0.1 -p "$PORT" -U yukipan -d postgres -c "CREATE DATABASE yukipan"
    fi
    echo "已启动: postgres://yukipan@127.0.0.1:$PORT/yukipan"
    ;;
  stop)
    if running; then
      "$PG_DIR/bin/pg_ctl" -D "$DATA_DIR" -m fast -w stop
      echo "已停止"
    else
      echo "未在运行"
    fi
    ;;
  destroy)
    running && "$PG_DIR/bin/pg_ctl" -D "$DATA_DIR" -m immediate stop || true
    rm -rf "$DEVDB_DIR"
    echo "已删除 $DEVDB_DIR"
    ;;
  *)
    echo "用法: $0 {setup|start|stop|destroy}" >&2
    exit 2
    ;;
esac
