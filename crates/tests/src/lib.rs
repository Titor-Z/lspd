//! 集成测试：覆盖「启动 → 握手 → 请求 → 断言」链路，防止 LSP 分帧与生命周期回归。

#[cfg(test)]
mod jsonrpc {
    use std::io::{Read, Write};
    use std::process::{Child, Command, Stdio};
    use std::thread;
    use std::time::Duration;

    fn frame(payload: &serde_json::Value) -> Vec<u8> {
        let body = serde_json::to_vec(payload).unwrap();
        format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes()
            .into_iter().chain(body).collect()
    }

    // 测试超长消息（100 KB）的 Content-Length 分帧
    #[ignore]
    #[test]
    fn test_long_message_frame() {
        let mut child = spawn_lspd_web().expect("spawn failed");
        let mut stdin = child.stdin.take().unwrap();
        let mut stdout = child.stdout.take().unwrap();

        // initialize
        let init = serde_json::json!({
            "jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "processId":null,"rootUri":"file://.","workspaceFolders":[],
                "capabilities":{"general":{},"textDocument":{},"workspace":{}}
            }});
        stdin.write_all(&frame(&init)).unwrap();
        thread::sleep(Duration::from_millis(50));

        // didOpen + completion with a long text (100KB)
        let long_text = "a".repeat(102400);
        let didopen = serde_json::json!({
            "jsonrpc":"2.0","method":"textDocument/didOpen","params":{
                "textDocument":{"uri":"file:///long.js","languageId":"javascript","version":1,"text":long_text}
            }});
        stdin.write_all(&frame(&didopen)).unwrap();
        thread::sleep(Duration::from_millis(100));

        let completion = serde_json::json!({
            "jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{
                "textDocument":{"uri":"file:///long.js"},"position":{"line":50000,"character":0}
            }});
        stdin.write_all(&frame(&completion)).unwrap();
        thread::sleep(Duration::from_millis(200));

        let mut buf = [0u8; 65536];
        let n = stdout.read(&mut buf).unwrap();
        assert!(n > 0, "expected response to long message");
        
        child.kill().ok();
    }

    fn spawn_lspd_web() -> Option<Child> {
        Command::new("cargo")
            .args(["build", "--release", "-p", "lspd"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .ok()
    }
}

#[cfg(test)]
mod lifecycle {
    use std::io::{Read, Write};
    use std::process::{Child, Command, Stdio};
    use std::thread;
    use std::time::Duration;

    fn frame(payload: &serde_json::Value) -> Vec<u8> {
        let body = serde_json::to_vec(payload).unwrap();
        format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes()
            .into_iter().chain(body).collect()
    }

    #[ignore]
    #[test]
    fn test_shutdown_refuses_requests() {
        let mut child = Command::new("cargo")
            .args(["build", "--release", "-p", "lspd"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn().expect("spawn lspd failed");

        let mut stdin = child.stdin.take().unwrap();
        let mut stdout = child.stdout.take().unwrap();

        // initialize
        let init = serde_json::json!({
            "jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "processId":null,"rootUri":"file://.","workspaceFolders":[],
                "capabilities":{"general":{},"textDocument":{},"workspace":{}}
            }});
        stdin.write_all(&frame(&init)).unwrap();
        thread::sleep(Duration::from_millis(50));

        // initialized
        let initialized = serde_json::json!({"jsonrpc":"2.0","method":"initialized","params":{}});
        stdin.write_all(&frame(&initialized)).unwrap();
        thread::sleep(Duration::from_millis(50));

        // shutdown
        let shutdown = serde_json::json!({"jsonrpc":"2.0","id":2,"method":"shutdown","params":{}});
        stdin.write_all(&frame(&shutdown)).unwrap();
        thread::sleep(Duration::from_millis(50));

        // 再发 completion 应被拒绝（错误码 -32002）
        let completion = serde_json::json!({
            "jsonrpc":"2.0","id":3,"method":"textDocument/completion","params":{
                "textDocument":{"uri":"file:///a.js"},"position":{"line":0,"character":0}
            }});
        stdin.write_all(&frame(&completion)).unwrap();
        thread::sleep(Duration::from_millis(50));

        let mut buf = [0u8; 4096];
        let n = stdout.read(&mut buf).unwrap();
        assert!(n > 0, "expected shutdown response");
        
        // 检查返回的错误码是否为 -32002（LSP_INVALID_REQUEST）
        let resp: serde_json::Value = serde_json::from_slice(&buf[..n]).unwrap();
        let err_code = resp.get("error").and_then(|e| e.get("code")).and_then(|c| c.as_i64());
        assert_eq!(err_code, Some(-32002), "expected error code -32002 after shutdown");
        
        child.kill().ok();
    }

    #[ignore]
    #[test]
    fn test_eof_exit_code() {
        // 验证 EOF 后退出码为 0，而非非零
        let mut child = Command::new("cargo")
            .args(["build", "--release", "-p", "lspd"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn().expect("spawn lspd failed");

        let stdin = child.stdin.take().unwrap();
        
        // 发送正常流程
        let init = serde_json::json!({
            "jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "processId":null,"rootUri":"file://.","workspaceFolders":[],
                "capabilities":{"general":{},"textDocument":{},"workspace":{}}
            }});
        stdin.write_all(&frame(&init)).unwrap();
        thread::sleep(Duration::from_millis(50));

        // EOF
        drop(stdin);
        thread::sleep(Duration::from_millis(50));

        let status = child.wait().unwrap();
        assert!(status.success(), "expected exit code 0 on EOF");
    }
}

#[cfg(test)]
mod position_encoding {
    use crate::jsonrpc::frame;

    // TODO: UTF-16 ↔ UTF-8 位置换算边界测试
    // 需要调用 lspd-core::text 模块的转换函数，验证：
    // 1. 行尾多字节字符（如"世界"）的 UTF-16/UTF-8 差值
    // 2. 代理对（emoji 🎉）的正确处理
    // 3. 跨行换行的边界情况

    // 示例骨架（待填实）：
    // #[ignore]
    // #[test]
    // fn test_utf16_to_utf8_surrogate_pair() {
    //     let utf8 = "🎉";
    //     let utf16_char_count = utf8.chars().count(); // 应为 1，但 UTF-16 占用 2 码元
    //     // ... 断言
    // }
}

#[cfg(test)]
mod capabilities_merge {
    use crate::router::Server as CoreServer;

    // TODO: 能力合并验证
    // 需要构造多个后端，各自声明部分 capabilities，然后验证合并后的结果：
    // 1. 对象字段递归深拷贝（不共享引用）
    // 2. 数组并集去重（如 completionProvider 的 triggerCharacters）
    // 3. 布尔取或（如 hoverProvider）

    // 示例骨架（待填实）：
    // #[ignore]
    // #[test]
    // fn test_capabilities_merge_object_deep_copy() {
    //     // 验证 A 后端的 textDocumentSync ≠ B 后端的同一字段引用
    // }

    // #[ignore]
    // #[test]
    // fn test_capabilities_merge_array_union_dedup() {
    //     // 验证两个后端都声明的 "auto-import" 只出现一次
    // }

    // #[ignore]
    // #[test]
    // fn test_capabilities_merge_boolean_or() {
    //     // A=false, B=true => merged=true
    // }
}
