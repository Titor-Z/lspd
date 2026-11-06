/**
 * 站点交互：复制命令、图表生长、标签页切换。全部为渐进增强，
 * 脚本不执行时内容依然完整可读。
 */

/** 复制命令到剪贴板，按钮给出短暂反馈。 */
function bindCopy(): void {
  document.querySelectorAll<HTMLButtonElement>('.copy').forEach((button) => {
    const label = button.textContent ?? '复制';

    button.addEventListener('click', async () => {
      try {
        await navigator.clipboard.writeText(button.dataset.copy ?? '');
        button.textContent = '已复制';
        button.classList.add('done');
      } catch {
        button.textContent = '失败';
      }
      window.setTimeout(() => {
        button.textContent = label;
        button.classList.remove('done');
      }, 1200);
    });
  });
}

/** 条形按组内最大值折算宽度，进入视口后生长一次。 */
function bindCharts(): void {
  const charts = Array.from(document.querySelectorAll<HTMLElement>('[data-chart]'));
  if (charts.length === 0) return;

  // 标记每组内的领先项：名字最像 lspd 的那一行
  charts.forEach((chart) => {
    const rows = Array.from(chart.querySelectorAll<HTMLElement>('.row'));
    rows
      .filter((row) => (row.querySelector('.name')?.textContent ?? '').trim().startsWith('lspd'))
      .forEach((row) => row.classList.add('is-lead'));
  });

  const grow = (chart: HTMLElement): void => {
    const rows = Array.from(chart.querySelectorAll<HTMLElement>('.row'));
    const max = rows.reduce((peak, row) => Math.max(peak, Number(row.dataset.bar ?? 0)), 0);
    if (max <= 0) return;

    rows.forEach((row) => {
      const value = Number(row.dataset.bar ?? 0);
      const fill = row.querySelector<HTMLElement>('.track i');
      if (!fill) return;
      // 最小 1%，否则小值会完全消失
      fill.style.width = `${Math.max(1, (value / max) * 100)}%`;
    });
  };

  if (!('IntersectionObserver' in window)) {
    charts.forEach(grow);
    return;
  }

  const observer = new IntersectionObserver(
    (entries) => {
      entries.forEach((entry) => {
        if (!entry.isIntersecting) return;
        grow(entry.target as HTMLElement);
        observer.unobserve(entry.target);
      });
    },
    { threshold: 0.2 },
  );

  charts.forEach((chart) => observer.observe(chart));
}

/** 编辑器标签页。 */
function bindTabs(): void {
  const tabs = Array.from(document.querySelectorAll<HTMLButtonElement>('.tabs button'));
  const panels = Array.from(document.querySelectorAll<HTMLElement>('[data-panel]'));
  if (tabs.length === 0) return;

  const select = (key: string): void => {
    tabs.forEach((tab) => tab.setAttribute('aria-selected', String(tab.dataset.tab === key)));
    panels.forEach((panel) => panel.classList.toggle('active', panel.dataset.panel === key));
  };

  tabs.forEach((tab) => tab.addEventListener('click', () => select(tab.dataset.tab ?? '')));

  // 键盘左右切换，符合 tablist 的惯例
  tabs.forEach((tab, index) => {
    tab.addEventListener('keydown', (event) => {
      const step = event.key === 'ArrowRight' ? 1 : event.key === 'ArrowLeft' ? -1 : 0;
      if (step === 0) return;
      event.preventDefault();
      const next = tabs[(index + step + tabs.length) % tabs.length];
      select(next.dataset.tab ?? '');
      next.focus();
    });
  });

  select(tabs[0].dataset.tab ?? '');
}

/** 页脚体积由构建脚本回填；这里只负责把值写进页面。 */
function bindFooter(): void {
  const target = document.querySelector<HTMLElement>('[data-size]');
  const stamp = document.documentElement.dataset.buildSize;
  if (target && stamp) target.textContent = stamp;
}

document.addEventListener('DOMContentLoaded', () => {
  bindCopy();
  bindCharts();
  bindTabs();
  bindFooter();
});
