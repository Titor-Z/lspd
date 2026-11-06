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

        let stdin = child.stdin.take().unwrap();
        
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
    // TODO: UTF-16 ↔ UTF-8 位置换算边界测试
}

#[cfg(test)]
mod capabilities_merge {
    // TODO: 能力合并验证（对象深拷贝、数组去并集、布尔取或）
}
