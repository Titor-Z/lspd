import './styles.css';

/** 把命令复制到剪贴板，并在按钮上给出短暂反馈。 */
function bindCopyButtons(): void {
  document.querySelectorAll<HTMLButtonElement>('.copy').forEach((button) => {
    const original = button.textContent ?? '复制';

    button.addEventListener('click', async () => {
      const text = button.dataset.copy ?? '';
      try {
        await navigator.clipboard.writeText(text);
        button.textContent = '已复制';
        button.classList.add('done');
        window.setTimeout(() => {
          button.textContent = original;
          button.classList.remove('done');
        }, 1200);
      } catch {
        button.textContent = '复制失败';
        window.setTimeout(() => {
          button.textContent = original;
        }, 1200);
      }
    });
  });
}

/** 按最大值折算比例，滚动进入视口后让条形生长。 */
function bindCharts(): void {
  const charts = Array.from(document.querySelectorAll<HTMLElement>('[data-chart]'));
  if (charts.length === 0) {
    return;
  }

  const grow = (chart: HTMLElement): void => {
    const rows = Array.from(chart.querySelectorAll<HTMLElement>('.row'));
    const max = rows.reduce(
      (peak, row) => Math.max(peak, Number(row.dataset.bar ?? '0')),
      0,
    );
    if (max <= 0) {
      return;
    }

    rows.forEach((row) => {
      const value = Number(row.dataset.bar ?? '0');
      const fill = row.querySelector<HTMLElement>('.track i');
      if (!fill) {
        return;
      }
      fill.style.width = `${Math.max(1.5, (value / max) * 100)}%`;
      row.classList.toggle('is-lspd', row.classList.contains('is-lspd'));
    });
  };

  const markStrong = (): void => {
    document.querySelectorAll<HTMLElement>('.row').forEach((row) => {
      const name = row.querySelector('.name');
      if (name?.classList.contains('strong')) {
        row.classList.add('is-lspd');
      }
    });
  };

  markStrong();

  if (!('IntersectionObserver' in window)) {
    charts.forEach(grow);
    return;
  }

  const observer = new IntersectionObserver(
    (entries) => {
      entries.forEach((entry) => {
        if (!entry.isIntersecting) {
          return;
        }
        grow(entry.target as HTMLElement);
        observer.unobserve(entry.target);
      });
    },
    { threshold: 0.25 },
  );

  charts.forEach((chart) => observer.observe(chart));
}

/** 编辑器配置的标签页切换。 */
function bindTabs(): void {
  const tabs = Array.from(document.querySelectorAll<HTMLButtonElement>('.tabs button'));
  const panels = Array.from(document.querySelectorAll<HTMLElement>('[data-panel]'));
  if (tabs.length === 0) {
    return;
  }

  const show = (key: string): void => {
    tabs.forEach((tab) => {
      tab.setAttribute('aria-selected', String(tab.dataset.tab === key));
    });
    panels.forEach((panel) => {
      panel.classList.toggle('active', panel.dataset.panel === key);
    });
  };

  tabs.forEach((tab) => {
    tab.addEventListener('click', () => show(tab.dataset.tab ?? ''));
  });

  show(tabs[0].dataset.tab ?? '');
}

/** 收尾处标注本站体积，构建后由构建脚本替换为真实值。 */
function bindFooter(): void {
  const size = document.querySelector<HTMLElement>('[data-size]');
  if (!size) {
    return;
  }
  const stamp = document.documentElement.dataset.buildSize;
  if (stamp) {
    size.textContent = stamp;
  }
}

document.addEventListener('DOMContentLoaded', () => {
  bindCopyButtons();
  bindCharts();
  bindTabs();
  bindFooter();
});
