// Bento Grid + Macaron + Bold Typography

document.addEventListener('DOMContentLoaded', () => {
  initEditorTabs();
  initChartAnimations();
});

function initEditorTabs() {
  const tabButtons = document.querySelectorAll('.editor-tab-btn') as NodeListOf<HTMLButtonElement>;
  
  tabButtons.forEach(btn => {
    btn.addEventListener('click', () => {
      tabButtons.forEach(b => b.classList.remove('active'));
      btn.classList.add('active');
      
      const editor = (btn as any).dataset.editor;
      renderCodeCards(editor);
    });
  });
  
  renderCodeCards('openidea');
}

function renderCodeCards(editor: string) {
  const container = document.getElementById('code-cards-area')!;
  
  const configs: Record<string, Array<{ name: string; cmd: string[]; lang?: string }>> = {
    openidea: [
      { name: 'Web Server', cmd: ['/usr/local/bin/lspd web'], lang: 'Command' },
      { name: 'TypeScript Server', cmd: ['/usr/local/bin/lspd ts'], lang: 'Command' }
    ],
    neovim: [
      { name: 'init.lua (web)', cmd: ['vim.lsp.start({\n  name = "lspd-web",\n  cmd = { lspd, "web" }\n})'], lang: 'Lua' },
      { name: 'init.lua (ts)', cmd: ['vim.lsp.start({\n  name = "lspd-ts",\n  cmd = { lspd, "ts" }\n})'], lang: 'Lua' }
    ],
    helix: [
      { name: 'languages.toml', cmd: ['[language-server.lspd-web]\ncommand = "lspd"\nargs = ["web"]'], lang: 'TOML' },
      { name: 'languages.toml (TS)', cmd: ['[language-server.lspd-ts]\ncommand = "lspd"\nargs = ["ts"]'], lang: 'TOML' }
    ],
    vscode: [
      { name: 'settings.json', cmd: ['{\n  "languageserver": {\n    "lspd-web": {\n      "command": "/usr/local/bin/lspd",\n      "args": ["web"]\n    }\n  }\n}'], lang: 'JSON' }
    ]
  };
  
  const configData = configs[editor] || configs.openidea;
  
  let html = '';
  configData.forEach((item, index) => {
    const timeoutDelay = index * 150;
    
    html += `<div class="code-card reveal-in" data-delay="${timeoutDelay}">
      <div class="code-header">
        <div class="code-dots">
          <span class="code-dot red"></span>
          <span class="code-dot yellow"></span>
          <span class="code-dot green"></span>
        </div>
        <div class="code-title">${item.name} · <span style="color:#8b8896;font-size:0.8rem">${item.lang}</span></div>
      </div>
      <pre><code>${escapeHtml(item.cmd.join('\n'))}</code></pre>
    </div>`;
  });
  
  container.innerHTML = html;
  
  // Trigger reveal animation
  requestAnimationFrame(() => {
    const elements = container.querySelectorAll('.reveal-in') as NodeListOf<Element>;
    elements.forEach(el => {
      const elTyped = el as HTMLElement;
      elTyped.style.opacity = '0';
      elTyped.style.transform = 'translateY(30px)';
    });
    
    setTimeout(() => {
      elements.forEach(el => {
        const elTyped = el as HTMLElement;
        elTyped.style.transition = 'opacity 0.6s cubic-bezier(0.215,0.61,0.355,1), transform 0.6s cubic-bezier(0.215,0.61,0.355,1)';
        elTyped.style.opacity = '1';
        elTyped.style.transform = 'translateY(0)';
      });
    }, 100);
  });
}

function escapeHtml(text: string): string {
  return text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

function initChartAnimations() {
  const bars = document.querySelectorAll('.bar-fill');
  
  const observer = new IntersectionObserver(entries => {
    entries.forEach(entry => {
      if (entry.isIntersecting) {
        const bar = entry.target as HTMLElement;
        const value = parseFloat(bar.dataset.value || '0');
        
        requestAnimationFrame(() => {
          bar.style.width = value + '%';
        });
        
        observer.unobserve(bar);
      }
    });
  }, { threshold: 0.5 });
  
  bars.forEach(bar => observer.observe(bar));
}

const cards = document.querySelectorAll('.card') as NodeListOf<HTMLElement>;
cards.forEach(card => {
  card.addEventListener('mousemove', e => {
    const rect = card.getBoundingClientRect();
    const x = e.clientX - rect.left;
    const y = e.clientY - rect.top;
    
    const centerX = rect.width / 2;
    const centerY = rect.height / 2;
    
    const rotateX = ((y - centerY) / centerY) * -6;
    const rotateY = ((x - centerX) / centerX) * 6;
    
    card.style.setProperty('--rx', `${rotateX}deg`);
    card.style.setProperty('--ry', `${rotateY}deg`);
  });
  
  card.addEventListener('mouseleave', () => {
    card.style.setProperty('--rx', `0deg`);
    card.style.setProperty('--ry', `0deg`);
  });
});
