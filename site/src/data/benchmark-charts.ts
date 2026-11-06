// 详细数据对比图表数据

export interface ChartDataPoint {
  label: string;
  value: number;
  unit: string;
  highlight?: boolean;
}

// 内存对比数据（Web 栈）
export const webMemoryChart: ChartDataPoint[] = [
  { label: 'lspd web', value: 8.9, unit: 'MB', highlight: true },
  { label: 'vscode-html', value: 137.6, unit: 'MB' },
  { label: 'vscode-css', value: 72.1, unit: 'MB' }
];

// TypeScript 栈性能对比
export const tsPerformanceChart: Array<{ metric: string; lspd: number; tsgo: number; tls: number; vtsls: number }> = [
  { 
    metric: '空闲内存 (MB)', 
    lspd: 3.7, tsgo: 9.6, tls: 51.3, vtsls: 60.2 
  },
  { 
    metric: '工作态内存 (MB)', 
    lspd: 16.7, tsgo: 9.9, tls: 57.5, vtsls: 64.9 
  },
  { 
    metric: '启动时间 (ms)', 
    lspd: 1.3, tsgo: 6.8, tls: 191.3, vtsls: 311.4 
  },
  { 
    metric: 'Hover P50 (ms)', 
    lspd: 0.23, tsgo: 0.27, tls: 1.39, vtsls: 2.19 
  },
  { 
    metric: 'Completion P50 (ms)', 
    lspd: 11.08, tsgo: 10.47, tls: 33.94, vtsls: 3.36 
  }
];
