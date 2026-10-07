//! YukiPan 配置解析: `config.toml` 读成朴素数据。
//!
//! 配置只放文档第 6、8 章钦定的机器常量与可调项; 读取入口是 [`Config::load`],
//! 它额外做一件事: 把 `data_root` 里的前导 `~` 展开成运行用户家目录。

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

const GIB: u64 = 1024 * 1024 * 1024;
const MIB: u64 = 1024 * 1024;

/// 后端运行配置。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// HTTP 监听地址。
    #[serde(default = "default_listen")]
    pub listen: SocketAddr,
    /// 数据根 (blobs/private/public/tmp 的父目录), 允许前导 `~`, 加载时展开。
    #[serde(default = "default_data_root")]
    pub data_root: PathBuf,
    /// PostgreSQL 连接串, 必填。
    pub database_url: String,
    /// Redis 连接串 (限流计数, 后续切片接入)。
    #[serde(default = "default_redis_url")]
    pub redis_url: String,
    /// 会话有效期 (小时), 过期需重新登录。
    #[serde(default = "default_session_ttl_hours")]
    pub session_ttl_hours: u64,
    /// 配额与单文件上限。
    #[serde(default)]
    pub quota: Quota,
    /// 访客空间的 TTL 与按 IP 限流。
    #[serde(default)]
    pub guest: Guest,
    /// 登录爆破计数 (文档第 5 章)。
    #[serde(default)]
    pub auth: Auth,
}

/// 登录爆破计数: 按 IP 固定窗口记失败, 超阈值 429 (文档第 5 章)。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Auth {
    /// 窗口内允许的最大失败次数, 达到后该 IP 一律 429 直到窗口过期。
    pub login_fail_max: u64,
    /// 失败计数窗口 (秒), 从第一次失败算起。
    pub login_fail_window_secs: u64,
}

impl Default for Auth {
    fn default() -> Self {
        Self {
            login_fail_max: 10,
            login_fail_window_secs: 600,
        }
    }
}

/// 访客空间防滥用: TTL 自动过期 + 按 IP 限流 (文档第 4、5 章)。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Guest {
    /// 访客文件过期时间 (小时), 到期删指向。
    pub ttl_hours: u64,
    /// 每 IP 每小时上传次数上限。
    pub upload_per_hour: u64,
    /// 每 IP 每小时上传字节上限。
    pub upload_bytes_per_hour: u64,
    /// 每 IP 每天上传字节上限。
    pub upload_bytes_per_day: u64,
}

impl Default for Guest {
    fn default() -> Self {
        Self {
            ttl_hours: 24,
            upload_per_hour: 10,
            upload_bytes_per_hour: 200 * MIB,
            upload_bytes_per_day: 500 * MIB,
        }
    }
}

/// 配额, 缺省值即文档第 6 章的预算。单位一律字节。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Quota {
    /// 私有区引用配额。
    pub private_limit: u64,
    /// 图床引用配额。
    pub images_limit: u64,
    /// 访客空间引用配额。
    pub guest_limit: u64,
    /// 盘上余量红线: 可用空间低于此值即拒收 (tmp/库/系统不计入配额)。
    pub reserve: u64,
    /// 访客单文件上限。
    pub guest_max_file: u64,
    /// 图床单文件上限。
    pub images_max_file: u64,
}

impl Default for Quota {
    fn default() -> Self {
        Self {
            private_limit: 14 * GIB,
            images_limit: 2 * GIB,
            guest_limit: 2 * GIB,
            reserve: 2 * GIB,
            guest_max_file: 50 * MIB,
            images_max_file: 20 * MIB,
        }
    }
}

/// 配置读取/解析失败。
#[derive(Debug, Error)]
pub enum ConfigError {
    /// 配置文件读不出来。
    #[error("读取配置文件 {path} 失败: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// TOML 不合法或字段类型不对。
    #[error("解析配置失败: {0}")]
    Parse(#[from] toml::de::Error),
    /// `data_root` 用了 `~` 但确定不了家目录。
    #[error("data_root 使用 `~` 但无法确定家目录")]
    NoHome,
}

impl Config {
    /// 从 TOML 文本解析 (纯解析, 不展开 `~`)。
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        Ok(toml::from_str(text)?)
    }

    /// 读文件并解析, 展开 `data_root` 的前导 `~`。
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let mut config = Self::parse(&text)?;
        if config.data_root.starts_with("~") {
            let home = std::env::home_dir().ok_or(ConfigError::NoHome)?;
            config.data_root = expand_home(config.data_root, &home);
        }
        Ok(config)
    }
}

/// 把前导 `~` 分量替换成 `home`; `~user` 形式不支持, 原样返回。
fn expand_home(path: PathBuf, home: &Path) -> PathBuf {
    match path.strip_prefix("~") {
        Ok(rest) => home.join(rest),
        Err(_) => path,
    }
}

fn default_listen() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 8516))
}

fn default_data_root() -> PathBuf {
    PathBuf::from("/var/lib/yukipan")
}

fn default_redis_url() -> String {
    "redis://127.0.0.1:6379".into()
}

fn default_session_ttl_hours() -> u64 {
    168
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn defaults_fill_when_only_required_given() {
        let c = Config::parse(r#"database_url = "postgres://x""#).unwrap();
        assert_eq!(c.listen, SocketAddr::from(([127, 0, 0, 1], 8516)));
        assert_eq!(c.data_root, PathBuf::from("/var/lib/yukipan"));
        assert_eq!(c.redis_url, "redis://127.0.0.1:6379");
        assert_eq!(c.session_ttl_hours, 168);
        assert_eq!(c.quota, Quota::default());
        assert_eq!(c.guest, Guest::default());
    }

    #[test]
    fn database_url_is_required() {
        assert!(Config::parse("").is_err());
    }

    #[test]
    fn unknown_field_is_rejected() {
        assert!(Config::parse("database_url = \"x\"\nbogus = 1").is_err());
    }

    #[test]
    fn quota_partial_override() {
        let c = Config::parse("database_url = \"x\"\n[quota]\nguest_max_file = 1024").unwrap();
        assert_eq!(c.quota.guest_max_file, 1024);
        assert_eq!(c.quota.private_limit, Quota::default().private_limit);
    }

    #[test]
    fn quota_defaults_match_doc() {
        let q = Quota::default();
        assert_eq!(q.private_limit, 14 * GIB);
        assert_eq!(q.images_limit, 2 * GIB);
        assert_eq!(q.guest_limit, 2 * GIB);
        assert_eq!(q.reserve, 2 * GIB);
        assert_eq!(q.guest_max_file, 50 * MIB);
        assert_eq!(q.images_max_file, 20 * MIB);
    }

    #[test]
    fn auth_defaults_match_doc() {
        let a = Auth::default();
        assert_eq!(a.login_fail_max, 10);
        assert_eq!(a.login_fail_window_secs, 600);
        let c = Config::parse(r#"database_url = "x""#).unwrap();
        assert_eq!(c.auth, a);
        let c = Config::parse("database_url = \"x\"\n[auth]\nlogin_fail_max = 3").unwrap();
        assert_eq!(c.auth.login_fail_max, 3);
        assert_eq!(c.auth.login_fail_window_secs, 600);
    }

    #[test]
    fn guest_defaults_match_doc() {
        let g = Guest::default();
        assert_eq!(g.ttl_hours, 24);
        assert_eq!(g.upload_per_hour, 10);
        assert_eq!(g.upload_bytes_per_hour, 200 * MIB);
        assert_eq!(g.upload_bytes_per_day, 500 * MIB);
    }

    #[test]
    fn expand_home_cases() {
        let home = Path::new("/home/yukipan");
        assert_eq!(
            expand_home(PathBuf::from("~/.YukiPan"), home),
            PathBuf::from("/home/yukipan/.YukiPan")
        );
        assert_eq!(
            expand_home(PathBuf::from("~"), home),
            PathBuf::from("/home/yukipan")
        );
        assert_eq!(
            expand_home(PathBuf::from("/abs/path"), home),
            PathBuf::from("/abs/path")
        );
        assert_eq!(
            expand_home(PathBuf::from("rel/path"), home),
            PathBuf::from("rel/path")
        );
        assert_eq!(
            expand_home(PathBuf::from("~user/x"), home),
            PathBuf::from("~user/x")
        );
    }

    #[test]
    fn load_reads_and_expands() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        write!(
            f,
            "database_url = \"postgres://x\"\ndata_root = \"~/pan-data\""
        )
        .unwrap();
        let home = std::env::home_dir().unwrap();
        let c = Config::load(f.path()).unwrap();
        assert_eq!(c.data_root, home.join("pan-data"));
    }

    #[test]
    fn load_missing_file() {
        let err = Config::load("/nonexistent/config.toml").unwrap_err();
        assert!(matches!(err, ConfigError::Read { .. }));
    }
}
