// ── Auth ────────────────────────────────────────────────────────
function apiKey() { return sessionStorage.getItem('api_key') || ''; }
function authHeaders() { return { 'Authorization': 'Bearer ' + apiKey(), 'Content-Type': 'application/json' }; }

function logout() {
  sessionStorage.removeItem('api_key');
  window.location.href = '/console';
}

(async function guard() {
  if (!apiKey()) { window.location.href = '/console'; return; }
  try {
    const res = await fetch('/health', { headers: authHeaders() });
    if (!res.ok) { logout(); return; }
  } catch { return; }
  init();
})();

// ── State ───────────────────────────────────────────────────────
let apps = [];
let selected = null;

async function init() {
  await loadBootstrap();
  await loadApps();
  setupSplitter();
  document.getElementById('deploy-btn').addEventListener('click', showDeployModal);
}

async function loadBootstrap() {
  try {
    const res = await fetch('/bootstrap/status', { headers: authHeaders() });
    if (!res.ok) return;
    const data = await res.json();
    document.getElementById('dns-suffix').textContent = data.dns_suffix || '…';

    const hr = await fetch('/health', { headers: authHeaders() });
    if (hr.ok) {
      document.getElementById('health-dot').className = 'status-dot rounded-full bg-emerald-500 inline-block';
      document.getElementById('health-text').textContent = 'Healthy';
    }
  } catch {}
}

async function loadApps() {
  try {
    const res = await fetch('/apps', { headers: authHeaders() });
    if (!res.ok) return;
    apps = await res.json();
  } catch { apps = []; }
  render();
}

// ── Render ──────────────────────────────────────────────────────
function render() {
  const list = document.getElementById('sidebar');
  if (apps.length === 0) {
    list.innerHTML = '<p class="text-xs text-gray-400 px-3 py-4 text-center">No applications yet</p>';
  } else {
    list.innerHTML = apps.map(a => `
      <button class="app-item w-full text-left px-3 py-2.5 rounded-lg mb-0.5 transition-colors hover:bg-gray-50 flex items-center gap-2.5" data-name="${esc(a.name)}">
        <span class="status-dot rounded-full ${a.status === 'running' ? 'bg-emerald-500' : 'bg-red-500'} inline-block flex-shrink-0"></span>
        <span class="text-sm font-medium truncate flex-1">${esc(a.name)}</span>
      </button>
    `).join('');

    list.querySelectorAll('.app-item').forEach(btn => {
      btn.addEventListener('click', () => selectApp(btn.dataset.name));
    });
  }

  if (selected) {
    const app = apps.find(a => a.name === selected);
    if (app) {
      selectApp(selected);
    } else {
      selected = null;
      showEmpty();
    }
  } else {
    showEmpty();
  }
}

function showEmpty() {
  document.getElementById('empty-state').classList.remove('hidden');
  document.getElementById('detail').classList.add('hidden');
  document.querySelectorAll('#sidebar .app-item').forEach(el => el.classList.remove('active'));
}

function selectApp(name) {
  const app = apps.find(a => a.name === name);
  if (!app) return;
  selected = name;

  document.querySelectorAll('#sidebar .app-item').forEach(el => el.classList.remove('active'));
  const btn = document.querySelector(`#sidebar [data-name="${CSS.escape(name)}"]`);
  if (btn) btn.classList.add('active');

  document.getElementById('empty-state').classList.add('hidden');
  document.getElementById('detail').classList.remove('hidden');

  document.getElementById('config-panel').innerHTML = `
    <div class="flex items-center justify-between mb-6">
      <div>
        <h2 class="text-xl font-bold">${esc(app.name)}</h2>
        <a href="http://${esc(app.hostname)}" target="_blank" class="text-indigo-600 hover:text-indigo-800 text-sm mono">${esc(app.hostname)} ↗</a>
      </div>
      <span class="inline-flex items-center gap-1.5 px-3 py-1 rounded-full text-xs font-medium ${app.status === 'running' ? 'bg-emerald-50 text-emerald-700' : 'bg-red-50 text-red-700'}">
        <span class="status-dot rounded-full ${app.status === 'running' ? 'bg-emerald-500' : 'bg-red-500'} inline-block"></span>
        ${app.status}
      </span>
    </div>
    <div class="space-y-4">
      <div>
        <p class="text-xs font-semibold text-gray-400 uppercase tracking-wider mb-2">Image</p>
        <div class="flex items-center gap-2 bg-gray-50 rounded-lg px-4 py-2.5">
          <span class="mono text-sm">${esc(app.image)}</span>
        </div>
      </div>
      <div>
        <p class="text-xs font-semibold text-gray-400 uppercase tracking-wider mb-2">Container</p>
        <p class="mono text-sm bg-gray-50 rounded-lg px-4 py-2.5">self-host-app-${esc(app.name)}</p>
      </div>
      <div class="grid grid-cols-2 gap-4">
        <div>
          <p class="text-xs font-semibold text-gray-400 uppercase tracking-wider mb-2">Hostname</p>
          <p class="mono text-sm bg-gray-50 rounded-lg px-4 py-2.5">${esc(app.hostname)}</p>
        </div>
        <div>
          <p class="text-xs font-semibold text-gray-400 uppercase tracking-wider mb-2">Status</p>
          <p class="mono text-sm bg-gray-50 rounded-lg px-4 py-2.5 capitalize">${app.status}</p>
        </div>
      </div>
    </div>
    <div class="mt-8 pt-6 border-t border-gray-100">
      <button id="remove-btn" class="text-sm text-red-500 hover:text-red-700 font-medium transition-colors">
        Remove application
      </button>
    </div>
  `;

  document.getElementById('remove-btn').addEventListener('click', () => removeApp(app.name));

  document.getElementById('logs-panel').innerHTML = `
    <div class="flex items-center gap-2 mb-4">
      <div class="flex gap-1.5">
        <span class="w-3 h-3 rounded-full bg-red-400"></span>
        <span class="w-3 h-3 rounded-full bg-yellow-400"></span>
        <span class="w-3 h-3 rounded-full bg-emerald-400"></span>
      </div>
      <span class="mono text-xs text-gray-400">logs — ${esc(app.name)}</span>
    </div>
    <div class="mono text-xs space-y-1">
      <p class="text-gray-500">$ self-host logs ${esc(app.name)} --follow</p>
      <p class="text-emerald-400">[--:--:--] Streaming logs from Docker…</p>
      <p class="text-gray-500 mt-2">— connected —</p>
    </div>
  `;
}

// ── Deploy ──────────────────────────────────────────────────────
function showDeployModal() {
  document.getElementById('deploy-modal').classList.remove('hidden');
  document.getElementById('deploy-name').focus();
  document.getElementById('deploy-error').classList.add('hidden');
}

function closeDeployModal() {
  document.getElementById('deploy-modal').classList.add('hidden');
}

async function deploy(e) {
  e.preventDefault();
  const name = document.getElementById('deploy-name').value.trim();
  const image = document.getElementById('deploy-image').value.trim();
  const errEl = document.getElementById('deploy-error');

  try {
    const res = await fetch('/apps', {
      method: 'POST',
      headers: authHeaders(),
      body: JSON.stringify({ name, image }),
    });
    if (!res.ok) {
      const body = await res.text();
      errEl.textContent = body;
      errEl.classList.remove('hidden');
      return;
    }
    document.getElementById('deploy-name').value = '';
    document.getElementById('deploy-image').value = '';
    closeDeployModal();
    selected = name;
    await loadApps();
  } catch (err) {
    errEl.textContent = 'Network error: ' + err.message;
    errEl.classList.remove('hidden');
  }
}

// ── Remove ──────────────────────────────────────────────────────
async function removeApp(name) {
  if (!confirm(`Remove "${name}"?`)) return;
  try {
    await fetch('/apps/' + encodeURIComponent(name), {
      method: 'DELETE',
      headers: authHeaders(),
    });
  } catch {}
  selected = null;
  await loadApps();
}

// ── Splitter ────────────────────────────────────────────────────
function setupSplitter() {
  const splitter = document.getElementById('splitter');
  const config = document.getElementById('config-panel');
  const logs = document.getElementById('logs-panel');
  let dragging = false;

  splitter.addEventListener('mousedown', (e) => {
    e.preventDefault();
    dragging = true;
    splitter.classList.add('dragging');
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';
  });

  document.addEventListener('mousemove', (e) => {
    if (!dragging) return;
    const detail = document.getElementById('detail');
    const rect = detail.getBoundingClientRect();
    const x = e.clientX - rect.left;
    const total = rect.width - splitter.offsetWidth;
    const pct = Math.max(15, Math.min(85, (x / total) * 100));
    config.style.flexBasis = pct + '%';
    logs.style.flex = '1';
  });

  document.addEventListener('mouseup', () => {
    if (!dragging) return;
    dragging = false;
    splitter.classList.remove('dragging');
    document.body.style.cursor = '';
    document.body.style.userSelect = '';
  });
}

// ── Utils ───────────────────────────────────────────────────────
function esc(s) { return s.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;'); }
