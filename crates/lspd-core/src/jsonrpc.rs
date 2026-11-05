//! LSP 的 JSON-RPC 分帧与消息解析。
//!
//! 设计要点：
//! - 入站参数以 `&RawValue` 借用正文，不构建 `serde_json::Value` 树。
//!   文档同步动辄数百 KB，构建值树意味着一次完整复制。
//! - 读缓冲区在多次消息间复用，只按需扩容，稳态下不再分配。
//! - 出站消息直接序列化进复用缓冲区，避免中间的 `String`。

use std::io::{self, BufRead, Write};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_json::value::RawValue;

use crate::message::{Id, Outgoing, ResponseError};

/// 入站消息。
///
/// 三种形态互斥，用枚举表达可让调用方在编译期穷尽处理，
/// 不需要在运行时判断"有没有 method""有没有 id"。
pub enum Message<'a> {
    /// 客户端请求服务端做事，必须应答
    Request {
        /// 请求标识，应答时原样回传
        id: Id,
        /// 方法名
        method: &'a str,
        /// 原始参数；方法本身可能不带参数
        params: Option<&'a RawValue>,
    },
    /// 客户端通知，不应答
    Notification {
        /// 方法名
        method: &'a str,
        /// 原始参数
        params: Option<&'a RawValue>,
    },
    /// 对服务端请求的应答
    Response {
        /// 对应请求标识
        id: Id,
        /// 成功结果
        result: Option<&'a RawValue>,
        /// 失败原因
        error: Option<&'a RawValue>,
    },
}

impl<'a> Message<'a> {
    /// 消息携带的方法名；应答没有方法名。
    pub fn method(&self) -> Option<&'a str> {
        match self {
            Self::Request { method, .. } | Self::Notification { method, .. } => Some(method),
            Self::Response { .. } => None,
        }
    }
}

/// 解析用的原始结构。字段全部可缺省，避免因个别字段缺失而拒绝整条消息。
#[derive(Deserialize)]
struct RawMessage<'a> {
    /// 请求或应答标识
    #[serde(default)]
    id: Option<Id>,
    /// 方法名
    #[serde(default)]
    method: Option<&'a str>,
    /// 参数
    #[serde(default, borrow)]
    params: Option<&'a RawValue>,
    /// 应答结果
    #[serde(default, borrow)]
    result: Option<&'a RawValue>,
    /// 应答错误
    #[serde(default, borrow)]
    error: Option<&'a RawValue>,
}

/// 解析一条消息正文。
///
/// 依据"有 method 即请求/通知，无 method 即应答"的规则判定形态。
pub fn parse(body: &[u8]) -> Result<Message<'_>, serde_json::Error> {
    let raw: RawMessage<'_> = serde_json::from_slice(body)?;

    Ok(match (raw.method, raw.id) {
        (Some(method), Some(id)) => Message::Request {
            id,
            method,
            params: raw.params,
        },
        (Some(method), None) => Message::Notification {
            method,
            params: raw.params,
        },
        (None, Some(id)) => Message::Response {
            id,
            result: raw.result,
            error: raw.error,
        },
        // 既无 method 也无 id：不是有效的 JSON-RPC 消息，按解析失败处理
        (None, None) => return Err(serde::de::Error::custom("缺少 method 与 id")),
    })
}

/// 分帧读取器。
///
/// LSP 以 `Content-Length` 头 + 空行界定正文，正文可以是任意 UTF-8 字节。
pub struct MessageReader<R> {
    /// 底层输入
    reader: R,
    /// 头部行缓冲，跨消息复用
    header: Vec<u8>,
    /// 正文字节缓冲，跨消息复用
    body: Vec<u8>,
}

impl<R: BufRead> MessageReader<R> {
    /// 包装一个实现了 `BufRead` 的输入源。
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            header: Vec::with_capacity(128),
            body: Vec::new(),
        }
    }

    /// 读取下一条消息正文；输入结束返回 `None`。
    pub fn next_message(&mut self) -> io::Result<Option<&[u8]>> {
        loop {
            let Some(length) = self.read_head()? else {
                return Ok(None);
            };
            // 只有头没有长度时（例如多余的换行），跳过继续读，而不是空转返回空消息
            if length == 0 {
                continue;
            }
            self.body.clear();
            self.body.resize(length, 0);
            self.reader.read_exact(&mut self.body)?;
            return Ok(Some(&self.body));
        }
    }

    /// 读取消息头，返回 `Content-Length`；流结束返回 `None`。
    fn read_head(&mut self) -> io::Result<Option<usize>> {
        let mut length: Option<usize> = None;

        loop {
            self.header.clear();
            let read = self.reader.read_until(b'\n', &mut self.header)?;
            if read == 0 {
                return Ok(None);
            }

            let line = trim_line_ending(&self.header);
            // 空行标志头部结束
            if line.is_empty() {
                return Ok(Some(length.unwrap_or(0)));
            }

            if let Some((name, value)) = split_header(line)
                && name.eq_ignore_ascii_case(b"content-length")
            {
                length = std::str::from_utf8(value)
                    .ok()
                    .and_then(|text| text.trim().parse().ok());
            }
        }
    }
}

/// 去掉行尾的 `\r\n` 或 `\n`。
fn trim_line_ending(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

/// 按第一个冒号切分头部行。
fn split_header(line: &[u8]) -> Option<(&[u8], &[u8])> {
    let position = line.iter().position(|byte| *byte == b':')?;
    Some((&line[..position], &line[position + 1..]))
}

/// 分帧写入器。
pub struct MessageWriter<W> {
    /// 底层输出
    writer: W,
    /// 序列化缓冲，跨消息复用
    buffer: Vec<u8>,
}

impl<W: Write> MessageWriter<W> {
    /// 包装一个输出目标。
    pub fn new(writer: W) -> Self {
        Self {
            writer,
            buffer: Vec::with_capacity(4096),
        }
    }

    /// 发送一条消息并立即刷新，保证客户端及时收到。
    pub fn send(&mut self, message: &Outgoing) -> io::Result<()> {
        self.buffer.clear();

        match message {
            Outgoing::Response { id, result } => match result {
                Ok(value) => {
                    let frame = ResponseFrame {
                        jsonrpc: "2.0",
                        id,
                        result: Some(value),
                        error: None,
                    };
                    serde_json::to_writer(&mut self.buffer, &frame)?;
                }
                Err(error) => {
                    let frame = ResponseFrame::<'_> {
                        jsonrpc: "2.0",
                        id,
                        result: None,
                        error: Some(error),
                    };
                    serde_json::to_writer(&mut self.buffer, &frame)?;
                }
            },
            Outgoing::Notification { method, params } => {
                let frame = NotificationFrame {
                    jsonrpc: "2.0",
                    method,
                    params,
                };
                serde_json::to_writer(&mut self.buffer, &frame)?;
            }
        }

        write!(self.writer, "Content-Length: {}\r\n\r\n", self.buffer.len())?;
        self.writer.write_all(&self.buffer)?;
        self.writer.flush()
    }
}

/// 应答帧。
#[derive(Serialize)]
struct ResponseFrame<'a> {
    /// 协议版本，固定 "2.0"
    jsonrpc: &'static str,
    /// 请求标识
    id: &'a Id,
    /// 成功结果，与 error 互斥
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<&'a Value>,
    /// 错误对象，与 result 互斥
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a ResponseError>,
}

/// 通知帧。
#[derive(Serialize)]
struct NotificationFrame<'a> {
    /// 协议版本，固定 "2.0"
    jsonrpc: &'static str,
    /// 方法名
    method: &'a str,
    /// 参数
    params: &'a Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 分帧写入后应能被重新解析回同一条消息。
    #[test]
    fn roundtrip_request() {
        let mut buffer = Vec::new();
        {
            let mut writer = MessageWriter::new(&mut buffer);
            writer
                .send(&Outgoing::notify(
                    "textDocument/publishDiagnostics",
                    json!({"uri": "file:///a"}),
                ))
                .unwrap();
        }
        let mut reader = MessageReader::new(std::io::Cursor::new(buffer));
        let body = reader.next_message().unwrap().unwrap();
        let message = parse(body).unwrap();
        assert_eq!(message.method(), Some("textDocument/publishDiagnostics"));
    }

    /// 头部含额外字段时仍应只取 Content-Length。
    #[test]
    fn tolerates_extra_headers() {
        let raw = b"Content-Type: application/vscode-jsonrpc; charset=utf-8\r\nContent-Length: 2\r\n\r\n{}";
        let mut reader = MessageReader::new(std::io::Cursor::new(raw.to_vec()));
        assert_eq!(reader.next_message().unwrap().unwrap(), b"{}");
    }

    /// 缺少 method 与 id 的正文按解析失败处理。
    #[test]
    fn rejects_invalid_message() {
        assert!(parse(b"{\"params\":{}}").is_err());
    }
}
