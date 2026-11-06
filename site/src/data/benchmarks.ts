// 数据来源：README、docs/Bench.md，统一来源避免不一致

export interface BenchmarkData {
  name: string;
  category: 'ts' | 'web';
  startup_ms: number;
  idle_mb: number;
  active_mb: number;
  hover_p50_ms: number;
  completion_p50_ms: number;
  diagnostic_p50_ms: number;
}

export const benchmarks: BenchmarkData[] = [
  // TypeScript 栈
  {
    name: "lspd ts",
    category: "ts",
    startup_ms: 1.3,
    idle_mb: 3.7,
    active_mb: 16.7,
    hover_p50_ms: 0.23,
    completion_p50_ms: 11.08,
    diagnostic_p50_ms: 0.28
  },
  {
    name: "tsgo (官方 TS 7)",
    category: "ts",
    startup_ms: 6.8,
    idle_mb: 9.6,
    active_mb: 9.9,
    hover_p50_ms: 0.27,
    completion_p50_ms: 10.47,
    diagnostic_p50_ms: 0.34
  },
  {
    name: "typescript-language-server",
    category: "ts",
    startup_ms: 191.3,
    idle_mb: 51.3,
    active_mb: 57.5,
    hover_p50_ms: 1.39,
    completion_p50_ms: 33.94,
    diagnostic_p50_ms: 1.03
  },
  {
    name: "vtsls",
    category: "ts",
    startup_ms: 311.4,
    idle_mb: 60.2,
    active_mb: 64.9,
    hover_p50_ms: 2.19,
    completion_p50_ms: 3.36,
    diagnostic_p50_ms: 1.22
  }
];

export const webComparisons: { name: string; idle_mb: number; active_mb: number }[] = [
  { name: "lspd web", idle_mb: 2.7, active_mb: 8.9 },
  { name: "vscode-html", idle_mb: 137.6, active_mb: 133.4 },
  { name: "vscode-css", idle_mb: 72.1, active_mb: 115.1 }
];
