//! 集成测试骨架（最小实现版）

#[cfg(test)]
mod basic {
    use std::process::Command;
    
    #[ignore]
    #[test]
    fn test_binary_exists_and_runs() {
        let bin = "/Users/titor/lsp/target/release/lspd";
        let output = Command::new(bin).args(["--version"]).output().expect("spawn failed");
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("lspd"));
    }
    
    #[ignore]
    #[test]
    fn test_web_subcommand_exists() {
        let bin = "/Users/titor/lsp/target/release/lspd";
        let output = Command::new(bin).args(["web", "--help"]).output().expect("spawn failed");
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("web"));
    }
    
    #[ignore]
    #[test]
    fn test_ts_subcommand_exists() {
        let bin = "/Users/titor/lsp/target/release/lspd";
        let output = Command::new(bin).args(["ts", "--help"]).output().expect("spawn failed");
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("typescript"));
    }
}
