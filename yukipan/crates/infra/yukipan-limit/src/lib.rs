//! YukiPan Redis 短时计数: 公开页风暴与登录爆破 (文档第 5 章)。
//!
//! 固定窗口计数: key 首次 INCR 时上 TTL, 窗口从第一次命中算起。
//!
//! 降级策略: Redis 连不上/操作失败时一律放行 (返回 0 / Ok), 只记日志。
//! 防风暴是保护层, 不该成为单点故障 — 网盘核心功能 (含登录) 不依赖 Redis;
//! 代价是 Redis 挂的时候限流与爆破计数同时失效, 这是有意的权衡。

use std::time::Duration;

use redis::aio::ConnectionManager;
use thiserror::Error;
use tracing::{info, warn};

/// 限流层错误。仅在显式需要时透出; 正常流程走降级放行, 不抛给调用方。
#[derive(Debug, Error)]
pub enum LimitError {
    /// Redis 操作失败。
    #[error("Redis 错误: {0}")]
    Redis(#[from] redis::RedisError),
}

/// INCR + 首次命中上 TTL 的原子 Lua (避免 INCR 后崩溃留下永不过期的 key)。
const INCR_LUA: &str = r#"
local c = redis.call('INCRBY', KEYS[1], ARGV[1])
if c == tonumber(ARGV[1]) then
    redis.call('PEXPIRE', KEYS[1], ARGV[2])
end
return c
"#;

/// 固定窗口计数器。Clone 廉价 (内部是连接管理器)。
#[derive(Debug, Clone)]
pub struct Limiter {
    /// None = 降级模式 (Redis 不可用), 所有操作放行。
    conn: Option<ConnectionManager>,
}

impl Limiter {
    /// 连 Redis; 连不上进入降级模式 (warn 告警), 不返回错误 — 见模块文档。
    ///
    /// 首次连接带超时: ConnectionManager 内部是无限退避重连, 不兜底的话
    /// Redis 挂了会卡住整个服务启动。
    pub async fn connect(redis_url: &str) -> Self {
        const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
        match redis::Client::open(redis_url) {
            Ok(client) => match tokio::time::timeout(CONNECT_TIMEOUT, client.get_connection_manager()).await {
                Err(_) => {
                    warn!("Redis 连接超时 ({CONNECT_TIMEOUT:?}), 限流降级为放行");
                    Self { conn: None }
                }
                Ok(Err(e)) => {
                    warn!("Redis 连接失败 ({e}), 限流降级为放行");
                    Self { conn: None }
                }
                Ok(Ok(conn)) => {
                    info!("Redis 已连接, 限流计数生效");
                    Self { conn: Some(conn) }
                }
            },
            Err(e) => {
                warn!("Redis URL 不合法 ({e}), 限流降级为放行");
                Self { conn: None }
            }
        }
    }

    /// 构造一个降级 (全放行) 实例, 测试与无 Redis 环境用。
    pub fn degraded() -> Self {
        Self { conn: None }
    }

    /// 动作前计数 +1, 返回窗口内当前计数; 调用方与阈值比较决定 429。
    pub async fn incr(&self, key: &str, window: Duration) -> u64 {
        self.incr_by(key, 1, window).await
    }

    /// 字节桶: 动作前/后加 amount, 返回窗口内当前总量。
    pub async fn incr_by(&self, key: &str, amount: u64, window: Duration) -> u64 {
        let Some(conn) = &self.conn else { return 0 };
        let mut conn = conn.clone();
        let script = redis::Script::new(INCR_LUA);
        let result: Result<u64, _> = script
            .key(key)
            .arg(amount)
            .arg(window.as_millis() as u64)
            .invoke_async(&mut conn)
            .await;
        match result {
            Ok(n) => n,
            Err(e) => {
                warn!("Redis 计数失败 ({e}), 本次放行");
                0
            }
        }
    }

    /// 只读当前计数 (登录爆破「超阈值拒绝下次尝试」的前置检查)。
    pub async fn get(&self, key: &str) -> u64 {
        let Some(conn) = &self.conn else { return 0 };
        let mut conn = conn.clone();
        match redis::cmd("GET")
            .arg(key)
            .query_async::<Option<String>>(&mut conn)
            .await
        {
            Ok(Some(s)) => s.parse().unwrap_or(0),
            Ok(None) => 0,
            Err(e) => {
                warn!("Redis 读计数失败 ({e}), 本次放行");
                0
            }
        }
    }

    /// key 的剩余生存期 (PTTL); key 不存在/没有 TTL/降级/失败一律 None。
    /// 429 响应的 Retry-After 取自这里。
    pub async fn ttl(&self, key: &str) -> Option<Duration> {
        let Some(conn) = &self.conn else { return None };
        let mut conn = conn.clone();
        match redis::cmd("PTTL")
            .arg(key)
            .query_async::<i64>(&mut conn)
            .await
        {
            // -2 = key 不存在, -1 = 没有 TTL (不该出现, 出现当没窗口处理)
            Ok(ms) if ms > 0 => Some(Duration::from_millis(ms as u64)),
            Ok(_) => None,
            Err(e) => {
                warn!("Redis 读 TTL 失败 ({e})");
                None
            }
        }
    }

    /// 清零 (登录成功后清掉该 IP 的失败计数)。
    pub async fn clear(&self, key: &str) {
        let Some(conn) = &self.conn else { return };
        let mut conn = conn.clone();
        if let Err(e) = redis::cmd("DEL")
            .arg(key)
            .query_async::<()>(&mut conn)
            .await
        {
            warn!("Redis 清计数失败 ({e})");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration as D;

    /// 降级实例一切放行。
    #[tokio::test]
    async fn degraded_passes_through() {
        let l = Limiter::degraded();
        assert_eq!(l.incr("k", D::from_secs(60)).await, 0);
        assert_eq!(l.incr_by("k", 100, D::from_secs(60)).await, 0);
        assert_eq!(l.get("k").await, 0);
        l.clear("k").await;
    }

    /// `YUKIPAN_TEST_REDIS_URL=redis://... cargo test -p yukipan-limit -- --ignored`
    #[tokio::test]
    #[ignore = "需要真实 Redis, 设 YUKIPAN_TEST_REDIS_URL 后加 --ignored 跑"]
    async fn incr_and_window() {
        let url = std::env::var("YUKIPAN_TEST_REDIS_URL").expect("缺少 YUKIPAN_TEST_REDIS_URL");
        let l = Limiter::connect(&url).await;
        let key = format!("test:{}", uuid::Uuid::new_v4());
        assert_eq!(l.incr(&key, D::from_secs(60)).await, 1);
        assert_eq!(l.incr(&key, D::from_secs(60)).await, 2);
        assert_eq!(l.incr_by(&key, 8, D::from_secs(60)).await, 10);
        assert_eq!(l.get(&key).await, 10);
        // ttl: 窗口在走, 剩余时间应在 (0, 60s] 之间
        let ttl = l.ttl(&key).await.expect("key 有 TTL");
        assert!(ttl.as_secs() <= 60 && ttl.as_millis() > 0);
        l.clear(&key).await;
        assert_eq!(l.get(&key).await, 0);
        assert_eq!(l.ttl(&key).await, None);
    }
}
