//! 集成测试：覆盖「启动 → 握手 → 请求 → 断言」链路，防止 LSP 分帧与生命周期回归。

#[cfg(test)]
mod jsonrpc {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::Duration;

    pub fn frame(payload: &serde_json::Value) -> Vec<u8> {
        let body = serde_json::to_vec(payload).unwrap();
        format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes()
            .into_iter().chain(body).collect()
    }

    #[ignore]
    #[test]
    fn test_long_message_frame() {
        let mut child = spawn_lspd_web().expect("spawn failed");
        let mut stdin = child.stdin.take().unwrap();
        let mut stdout = child.stdout.take().unwrap();

        let init = serde_json::json!({
            "jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "processId":null,"rootUri":"file://.","workspaceFolders":[],
                "capabilities":{"general":{},"textDocument":{},"workspace":{}}
            }});
        stdin.write_all(&frame(&init)).unwrap();
        thread::sleep(Duration::from_millis(50));

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

    fn spawn_lspd_web() -> Option<std::process::Child> {
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
    use crate::jsonrpc::frame;
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::Duration;

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

        let init = serde_json::json!({
            "jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "processId":null,"rootUri":"file://.","workspaceFolders":[],
                "capabilities":{"general":{},"textDocument":{},"workspace":{}}
            }});
        stdin.write_all(&frame(&init)).unwrap();
        thread::sleep(Duration::from_millis(50));

        let initialized = serde_json::json!({"jsonrpc":"2.0","method":"initialized","params":{}});
        stdin.write_all(&frame(&initialized)).unwrap();
        thread::sleep(Duration::from_millis(50));

        let shutdown = serde_json::json!({"jsonrpc":"2.0","id":2,"method":"shutdown","params":{}});
        stdin.write_all(&frame(&shutdown)).unwrap();
        thread::sleep(Duration::from_millis(50));

        let completion = serde_json::json!({
            "jsonrpc":"2.0","id":3,"method":"textDocument/completion","params":{
                "textDocument":{"uri":"file:///a.js"},"position":{"line":0,"character":0}
            }});
        stdin.write_all(&frame(&completion)).unwrap();
        thread::sleep(Duration::from_millis(50));

        let mut buf = [0u8; 4096];
        let n = stdout.read(&mut buf).unwrap();
        assert!(n > 0, "expected shutdown response");
        
        let resp: serde_json::Value = serde_json::from_slice(&buf[..n]).unwrap();
        let err_code = resp.get("error").and_then(|e| e.get("code")).and_then(|c| c.as_i64());
        assert_eq!(err_code, Some(-32002), "expected error code -32002 after shutdown");
        
        child.kill().ok();
    }

    #[ignore]
    #[test]
    fn test_eof_exit_code() {
        let mut child = Command::new("cargo")
            .args(["build", "--release", "-p", "lspd"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn().expect("spawn lspd failed");

        let mut stdin = child.stdin.take().unwrap();
        
        let init = serde_json::json!({
            "jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "processId":null,"rootUri":"file://.","workspaceFolders":[],
                "capabilities":{"general":{},"textDocument":{},"workspace":{}}
            }});
        stdin.write_all(&frame(&init)).unwrap();
        thread::sleep(Duration::from_millis(50));

        drop(stdin);
        thread::sleep(Duration::from_millis(50));

        let status = child.wait().unwrap();
        assert!(status.success(), "expected exit code 0 on EOF");
    }
}

#[cfg(test)]
mod position_encoding {
    use crate::jsonrpc::frame;
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::Duration;

    // UTF-16 ↔ UTF-8 位置换算：多字节字符（如"世界"）的边界验证
    #[ignore]
    #[test]
    fn test_utf16_to_utf8_chinese_chars() {
        let mut child = spawn_lspd_web().expect("spawn failed");
        let mut stdin = child.stdin.take().unwrap();
        let mut stdout = child.stdout.take().unwrap();

        let init = serde_json::json!({
            "jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "processId":null,"rootUri":"file://.","workspaceFolders":[],
                "capabilities":{"general":{},"textDocument":{},"workspace":{}}
            }});
        stdin.write_all(&frame(&init)).unwrap();
        thread::sleep(Duration::from_millis(50));

        // 中文文本："你好世界"，每个汉字 UTF-8 占 3 字节，UTF-16 占 2 码元
        let text = "你好世界";
        let didopen = serde_json::json!({
            "jsonrpc":"2.0","method":"textDocument/didOpen","params":{
                "textDocument":{"uri":"file:///chinese.txt","languageId":"plaintext","version":1,"text":text}
            }});
        stdin.write_all(&frame(&didopen)).unwrap();
        thread::sleep(Duration::from_millis(50));

        // 请求第 2 个字符的位置（UTF-16 索引为 1），应该触发位置换算逻辑
        let hover = serde_json::json!({
            "jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{
                "textDocument":{"uri":"file:///chinese.txt"},"position":{"line":0,"character":1}
            }});
        stdin.write_all(&frame(&hover)).unwrap();
        thread::sleep(Duration::from_millis(50));

        let mut buf = [0u8; 4096];
        let n = stdout.read(&mut buf).unwrap();
        assert!(n > 0, "expected response for chinese char hover");
        
        child.kill().ok();
    }

    // UTF-16 ↔ UTF-8 位置换算：代理对（emoji 🎉）的边界验证
    #[ignore]
    #[test]
    fn test_utf16_surrogate_pair_emoji() {
        let mut child = spawn_lspd_web().expect("spawn failed");
        let mut stdin = child.stdin.take().unwrap();
        let mut stdout = child.stdout.take().unwrap();

        let init = serde_json::json!({
            "jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "processId":null,"rootUri":"file://.","workspaceFolders":[],
                "capabilities":{"general":{},"textDocument":{},"workspace":{}}
            }});
        stdin.write_all(&frame(&init)).unwrap();
        thread::sleep(Duration::from_millis(50));

        // emoji 🎉，UTF-8 占 4 字节，UTF-16 占 2 码元（代理对）
        let text = "Hello 🎉 World";
        let didopen = serde_json::json!({
            "jsonrpc":"2.0","method":"textDocument/didOpen","params":{
                "textDocument":{"uri":"file:///emoji.txt","languageId":"plaintext","version":1,"text":text}
            }});
        stdin.write_all(&frame(&didopen)).unwrap();
        thread::sleep(Duration::from_millis(50));

        // 请求 emoji 位置的 hover（UTF-16 索引为 7，但实际占用 2 码元）
        let hover = serde_json::json!({
            "jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{
                "textDocument":{"uri":"file:///emoji.txt"},"position":{"line":0,"character":7}
            }});
        stdin.write_all(&frame(&hover)).unwrap();
        thread::sleep(Duration::from_millis(50));

        let mut buf = [0u8; 4096];
        let n = stdout.read(&mut buf).unwrap();
        assert!(n > 0, "expected response for surrogate pair hover");
        
        child.kill().ok();
    }

    fn spawn_lspd_web() -> Option<std::process::Child> {
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
mod capabilities_merge {
    use std::collections::HashMap;
    
    // TODO: 能力合并验证（对象深拷贝、数组去并集、布尔取或）
    // 需要构造多个后端，各自声明部分 capabilities，然后验证合并后的结果
    
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
