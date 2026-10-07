//! 出参统一信封: `{ success, data, message }` (文档第 7 章)。

use serde::Serialize;

/// 统一信封。三个键恒定出现; 失败时 `data` 为 null, 成功时 `message` 为空串。
#[derive(Debug, Serialize)]
pub struct Envelope<T> {
    success: bool,
    data: Option<T>,
    message: String,
}

impl<T> Envelope<T> {
    /// 成功。
    pub fn ok(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            message: String::new(),
        }
    }
}

impl Envelope<()> {
    /// 失败。
    pub fn err(message: impl Into<String>) -> Self {
        Self {
            success: false,
            data: None,
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_shape() {
        let v = serde_json::to_value(Envelope::ok(serde_json::json!({"id": 1}))).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"success": true, "data": {"id": 1}, "message": ""})
        );
    }

    #[test]
    fn err_shape() {
        let v = serde_json::to_value(Envelope::err("不行")).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"success": false, "data": null, "message": "不行"})
        );
    }
}
