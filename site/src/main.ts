import './styles/main.css';

// Tab 切换 + 代码块 stagger 动画
function initTabs() {
  const tabButtons = document.querySelectorAll('.tabs button') as NodeListOf<HTMLButtonElement>;
  
  tabButtons.forEach(btn => {
    btn.addEventListener('click', () => {
      // Reset all tabs
      tabButtons.forEach(b => b.setAttribute('aria-selected', 'false'));
      btn.setAttribute('aria-selected', 'true');
      
      const tabId = (btn as any).dataset.tab;
      if (!tabId) return;
      
      // Hide all panels
      document.querySelectorAll('[data-tab-panel]').forEach(p => p.classList.remove('visible'));
      
      // Show target panel with stagger
      setTimeout(() => {
        const panel = document.querySelector(`[data-tab-panel="${tabId}"]`);
        if (panel) {
          renderEditorConfig(tabId, panel);
          panel.classList.add('visible');
        }
      }, 150);
    });
    
    // Initialize first tab on load
    if ((btn as any).dataset.tab === 'openidea') {
      btn.setAttribute('aria-selected', 'true');
    }
  });
}

// 渲染单个编辑器的配置，带 stagger
function renderEditorConfig(key: string, panel: Element) {
  const configs: Record<string, string> = {
    openidea: `# Settings → Languages & Frameworks → Language Servers → +

【Web】
Name: lspd web
Command: /usr/local/bin/lspd web
Languages: HTML, CSS, SCSS, Less, Sass, JSON

【TypeScript】
Name: lspd ts
Command: /usr/local/bin/lspd ts
Languages: TypeScript, JavaScript`,
    
    neovim: `-- init.lua
local lspd = "/usr/local/bin/lspd"

vim.lsp.start({
  name = "lspd-web",
  cmd = { lspd, "web" }
}, { bufnr = vim.api.nvim_get_current_buf() })

vim.lsp.start({
  name = "lspd-ts",
  cmd = { lspd, "ts" }
}, { bufnr = vim.api.nvim_get_current_buf() })`,
    
    helix: `# ~/.config/helix/languages.toml
[language-server.lspd-web]
command = "lspd"
args = ["web"]

[[language]]
name = "html"
language-servers = ["lspd-web"]

[language-server.lspd-ts]
command = "lspd"
args = ["ts"]

[[language]]
name = "typescript"
language-servers = ["lspd-ts"]`,
    
    vscode: `{
  // settings.json（需配合 LSP 扩展）
  "languageserver": {
    "lspd-web": {
      "command": "/usr/local/bin/lspd",
      "args": ["web"],
      "filetypes": ["html","css","scss"]
    },
    "lspd-ts": {
      "command": "/usr/local/bin/lspd",
      "args": ["ts"],
      "filetypes": ["typescript","javascript"]
    }
  }
}`
  };
  
  const code = escapeHtml(configs[key]);
  const lines = code.split('\n').filter(l => l.trim());
  
  const copyBtnHtml = '<button class="copy-btn" data-target="#code-' + key + '">复制</button>';
  let preHtml = '<pre><code id="code-' + key + '">';
  
  lines.forEach((line, i) => {
    const delayClass = 'stagger-' + Math.min(5, Math.floor(i / 3) + 1);
    preHtml += '<span class="' + delayClass + '">' + line + '</span>\n';
  });
  
  preHtml += '</code></pre>';
  
  panel.innerHTML = '<div class="code-block">' + copyBtnHtml + preHtml + '</div>';
  
  // Setup copy button
  setTimeout(() => {
    const btn = panel.querySelector('.copy-btn') as HTMLElement;
    if (btn) {
      btn.addEventListener('click', async () => {
        const targetId = (btn.dataset.target || '').replace('#', '');
        const codeEl = document.getElementById(targetId);
        if (!codeEl) return;
        
        await navigator.clipboard.writeText(codeEl.textContent || '');
        
        const oldText = btn.textContent;
        btn.textContent = '✓';
        btn.style.borderColor = '#4ade80';
        
        setTimeout(() => {
          btn.textContent = oldText;
          btn.style.borderColor = '';
        }, 1200);
      });
    }
  }, 100);
}

function escapeHtml(text: string): string {
  return text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

// Intersection Observer - 触发动画
function initReveals() {
  if (!('IntersectionObserver' in window)) return;
  
  const observer = new IntersectionObserver(entries => {
    entries.forEach(entry => {
      if (entry.isIntersecting) {
        entry.target.classList.add('visible');
      }
    });
  }, { threshold: 0.15 });
  
  Array.from(document.querySelectorAll('[data-reveal]')).forEach((el: Element) => {
    observer.observe(el);
  });
}

// Hero 打字机
function initHeroTypewriter() {
  const heroCode = document.querySelector('.hero-code');
  if (!heroCode) return;
  
  const text = 'lspd web';
  let index = 0;
  const cmdText = heroCode.querySelector('.cmd-text') as HTMLElement | null;
  if (!cmdText) return;
  
  const interval = setInterval(() => {
    cmdText.textContent += text[index];
    index++;
    if (index >= text.length) clearInterval(interval);
  }, 100);
}

// Bar 动画 - 进入视口后开始生长
function initBarAnimations() {
  const bars = document.querySelectorAll('.bar-fill');
  const observer = new IntersectionObserver(entries => {
    entries.forEach(entry => {
      if (entry.isIntersecting) {
        const bar = entry.target as HTMLElement;
        const value = parseFloat(bar.dataset.value || '0');
        bar.style.width = value + '%';
        bar.classList.add('bounce');
      }
    });
  }, { threshold: 0.5 });
  
  bars.forEach(bar => observer.observe(bar));
}

document.addEventListener('DOMContentLoaded', () => {
  initHeroTypewriter();
  initTabs();
  initReveals();
  initBarAnimations();
});
