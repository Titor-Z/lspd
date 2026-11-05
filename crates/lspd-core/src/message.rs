//! JSON-RPC 消息的表示与序列化。
//!
//! 这一层只负责"长什么样"，不负责读写（读写见传输层）。
//! 入站消息在传输层以借用形态解析，出站消息在此聚合成可序列化的值。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC 消息标识。协议允许数字与字符串两种形态，客户端可任选其一。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Id {
    /// 数字标识，多数客户端使用自增整数
    Number(i64),
    /// 字符串标识
    String(String),
}

/// 响应中的错误对象。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseError {
    /// 错误码，取值见 `ErrorCode`
    pub code: i64,
    /// 面向用户的可读描述
    pub message: String,
    /// 附加数据，协议允许省略
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// 协议规定的错误码。
pub mod error_code {
    /// 收到无法解析的 JSON
    pub const PARSE_ERROR: i64 = -32700;
    /// 结构合法但不是有效的请求对象
    pub const INVALID_REQUEST: i64 = -32600;
    /// 方法不存在或该后端不处理该方法
    pub const METHOD_NOT_FOUND: i64 = -32601;
    /// 参数不合法
    pub const INVALID_PARAMS: i64 = -32602;
    /// 服务端内部错误
    pub const INTERNAL_ERROR: i64 = -32603;
    /// 尚未收到 initialize，服务端不接受请求
    pub const SERVER_NOT_INITIALIZED: i64 = -32002;
    /// 请求被客户端取消
    pub const REQUEST_CANCELLED: i64 = -32800;
}

/// 出站消息。事件循环处理后统一写入 stdout。
#[derive(Debug, Clone)]
pub enum Outgoing {
    /// 请求的应答
    Response {
        /// 对应入站请求的标识
        id: Id,
        /// 成功结果或错误对象
        result: Result<Value, ResponseError>,
    },
    /// 服务端主动通知，如诊断、日志、进度
    Notification {
        /// 方法名
        method: String,
        /// 参数
        params: Value,
    },
}

impl Outgoing {
    /// 构造成功应答。
    pub fn ok(id: Id, result: Value) -> Self {
        Self::Response {
            id,
            result: Ok(result),
        }
    }

    /// 构造错误应答。
    pub fn err(id: Id, code: i64, message: impl Into<String>) -> Self {
        Self::Response {
            id,
            result: Err(ResponseError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }

    /// 构造通知。
    pub fn notify(method: impl Into<String>, params: Value) -> Self {
        Self::Notification {
            method: method.into(),
            params,
        }
    }
}
