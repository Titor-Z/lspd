//! Dockerfile 轻量解析。
//!
//! 按行识别指令、注释与续行，不构建完整 AST；足够支撑 hover/completion/基础诊断。

use lspd_core::text::Span;

/// Dockerfile 指令节点。
#[derive(Debug, Clone)]
pub struct Instruction {
    /// 指令名（大写）
    pub name: String,
    /// 指令名区间
    pub name_span: Span,
}

/// 解析结果。
#[derive(Debug, Clone, Default)]
pub struct ParseResult {
    /// 识别到的指令
    pub instructions: Vec<Instruction>,
}

/// 解析 Dockerfile。
pub fn parse(source: &str) -> ParseResult {
    let mut instructions = Vec::new();
    let mut offset = 0usize;
    let mut continued = false;

    for raw_line in source.split_inclusive('\n') {
        let line = raw_line.trim_end_matches(['\n', '\r']);
        if !continued {
            if let Some(inst) = parse_instruction(line, offset) {
                instructions.push(inst);
            }
        }
        continued = line.trim_end().ends_with('\\');
        offset += raw_line.len();
    }

    // 处理无末尾换行的最后一行已由 split_inclusive 覆盖；空文件无事发生。
    ParseResult { instructions }
}

/// 解析一行开头的指令。
fn parse_instruction(line: &str, line_offset: usize) -> Option<Instruction> {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }

    let indent = line.len() - trimmed.len();
    let name_len = trimmed
        .bytes()
        .take_while(|b| b.is_ascii_alphabetic())
        .count();
    if name_len == 0 {
        return None;
    }

    let name = &trimmed[..name_len];
    let rest = &trimmed[name_len..];
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }

    let start = line_offset + indent;
    Some(Instruction {
        name: name.to_ascii_uppercase(),
        name_span: Span::new(start, start + name_len),
    })
}

/// 查找 offset 所在指令名。
pub fn instruction_at(result: &ParseResult, offset: u32) -> Option<&Instruction> {
    result
        .instructions
        .iter()
        .find(|inst| inst.name_span.contains(offset))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_instructions() {
        let r = parse("FROM node:20\nRUN npm ci\n");
        assert_eq!(r.instructions.len(), 2);
        assert_eq!(r.instructions[0].name, "FROM");
        assert_eq!(r.instructions[1].name, "RUN");
    }

    #[test]
    fn skips_comments_and_continuations() {
        let r = parse("# comment\nRUN echo a\\\n  && echo b\nCOPY . .\n");
        assert_eq!(r.instructions.len(), 2);
        assert_eq!(r.instructions[0].name, "RUN");
        assert_eq!(r.instructions[1].name, "COPY");
    }
}
