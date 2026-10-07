#!/usr/bin/env bash
# YukiPan 开发用 Redis: 源码便携构建, 不进系统。
# 一切都在 yukipan/target/devredis/ 下 (target 已 gitignore, cargo clean 也会清掉)。
#
#   dev/devredis.sh setup     下载源码 (校验 sha256) 并 make 构建 (幂等)
#   dev/devredis.sh start     启动, 监听 127.0.0.1:56379, 无持久化 (短时计数不需要)
#   dev/devredis.sh stop      停止
#   dev/devredis.sh destroy   停止并删除整个 devredis 目录 (完全移除)
set -euo pipefail

DEVREDIS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/target/devredis"
VERSION=8.2.1
SHA256=e2c1cb9dd4180a35b943b85dfc7dcdd42566cdbceca37d0d0b14c21731582d3e
TARBALL_URL="https://download.redis.io/releases/redis-$VERSION.tar.gz"
SRC_DIR="$DEVREDIS_DIR/redis-$VERSION"
SERVER="$SRC_DIR/src/redis-server"
CLI="$SRC_DIR/src/redis-cli"
PID_FILE="$DEVREDIS_DIR/redis.pid"
LOG="$DEVREDIS_DIR/redis.log"
PORT=56379

running() {
  [ -f "$PID_FILE" ] && kill -0 "$(cat "$PID_FILE")" 2>/dev/null
}

case "${1:-}" in
  setup)
    if [ ! -x "$SERVER" ]; then
      mkdir -p "$DEVREDIS_DIR"
      echo "下载 Redis $VERSION 源码..."
      curl -fL "$TARBALL_URL" -o "$DEVREDIS_DIR/redis.tar.gz"
      echo "$SHA256  $DEVREDIS_DIR/redis.tar.gz" | sha256sum -c -
      tar -xzf "$DEVREDIS_DIR/redis.tar.gz" -C "$DEVREDIS_DIR"
      rm -f "$DEVREDIS_DIR/redis.tar.gz"
      echo "编译 (make -j$(nproc))..."
      make -C "$SRC_DIR" -j"$(nproc)" >/dev/null
    fi
    echo "setup 完成。启动: $0 start"
    ;;
  start)
    running && { echo "已在运行 (pid $(cat "$PID_FILE"))"; exit 0; }
    [ -x "$SERVER" ] || { echo "未构建, 先跑: $0 setup" >&2; exit 1; }
    # 无持久化: 限流计数是短时数据, 重启丢了无所谓
    "$SERVER" --port "$PORT" --bind 127.0.0.1 --daemonize yes \
      --pidfile "$PID_FILE" --dir "$DEVREDIS_DIR" --logfile "$LOG" \
      --save '' --appendonly no
    for _ in $(seq 1 50); do
      "$CLI" -p "$PORT" ping 2>/dev/null | grep -qx PONG && break
      sleep 0.1
    done
    echo "已启动: redis://127.0.0.1:$PORT"
    ;;
  stop)
    if running; then
      "$CLI" -p "$PORT" shutdown nosave 2>/dev/null || kill "$(cat "$PID_FILE")"
      rm -f "$PID_FILE"
      echo "已停止"
    else
      echo "未在运行"
    fi
    ;;
  destroy)
    running && { "$CLI" -p "$PORT" shutdown nosave 2>/dev/null || kill "$(cat "$PID_FILE")"; } || true
    rm -rf "$DEVREDIS_DIR"
    echo "已删除 $DEVREDIS_DIR"
    ;;
  *)
    echo "用法: $0 {setup|start|stop|destroy}" >&2
    exit 2
    ;;
esac
