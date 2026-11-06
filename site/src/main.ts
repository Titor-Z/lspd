// Bento Grid + 马卡龙配色官网逻辑

document.addEventListener('DOMContentLoaded', () => {
  // Tab switching with stagger animation
  initTabs();
  
  // Hover tilt effect for cards
  initTiltEffect();
});

function initTabs() {
  const tabs = document.querySelectorAll('.tab-btn') as NodeListOf<HTMLButtonElement>;
  
  tabs.forEach(tab => {
    tab.addEventListener('click', () => {
      // Remove active class from all tabs
      tabs.forEach(t => {
        t.classList.remove('active');
        t.setAttribute('aria-selected', 'false');
      });
      
      // Add active to clicked tab
      tab.classList.add('active');
      tab.setAttribute('aria-selected', 'true');
      
      const tabId = (tab as any).dataset.tab;
      if (!tabId) return;
      
      // Show content
      renderTabContent(tabId);
    });
  });
  
  // Initialize first tab
  renderTabContent('openidea');
}

function renderTabContent(key: string) {
  const container = document.getElementById('tab-content')!;
  
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
  
  let html = '<pre><code>';
  lines.forEach((line, i) => {
    const delayClass = 'stagger-' + Math.min(5, Math.floor(i / 3) + 1);
    html += `<span class="${delayClass}">${line}</span>\n`;
  });
  html += '</code></pre>';
  
  container.innerHTML = html;
  
  // Reveal animation
  container.style.display = 'block';
  container.style.opacity = '0';
  container.style.transform = 'translateY(20px)';
  
  requestAnimationFrame(() => {
    container.style.transition = 'opacity 0.6s ease-out, transform 0.6s cubic-bezier(0.215, 0.61, 0.355, 1)';
    container.style.opacity = '1';
    container.style.transform = 'translateY(0)';
  });
}

function escapeHtml(text: string): string {
  return text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

// 3D Tilt Effect
function initTiltEffect() {
  const cards = document.querySelectorAll('.card') as NodeListOf<HTMLElement>;
  
  cards.forEach(card => {
    card.addEventListener('mousemove', (e) => {
      const rect = card.getBoundingClientRect();
      const x = e.clientX - rect.left;
      const y = e.clientY - rect.top;
      
      const centerX = rect.width / 2;
      const centerY = rect.height / 2;
      
      const rotateX = ((y - centerY) / centerY) * -5; // max -5 to 5 deg
      const rotateY = ((x - centerX) / centerX) * 5;
      
      card.style.transform = `perspective(800px) rotateX(${rotateX}deg) rotateY(${rotateY}deg)`;
    });
    
    card.addEventListener('mouseleave', () => {
      card.style.transform = 'perspective(800px) rotateX(0) rotateY(0)';
    });
  });
}
