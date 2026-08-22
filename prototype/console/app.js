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
let logAbort = null;

async function init() {
  await loadBootstrap();
  await loadApps();
  setupSplitter();
  document.getElementById('deploy-btn').addEventListener('click', showDeployModal);
  document.getElementById('deploy-close').addEventListener('click', closeDeployModal);
  document.getElementById('deploy-cancel').addEventListener('click', closeDeployModal);
  document.getElementById('deploy-form').addEventListener('submit', deploy);
  document.addEventListener('keydown', handleDialogKeydown);
}

async function loadBootstrap() {
  try {
    const res = await fetch('/bootstrap/status', { headers: authHeaders() });
    if (!res.ok) return;
    const data = await res.json();
    document.getElementById('dns-suffix').textContent = data.dns_suffix || '…';

    const hr = await fetch('/health', { headers: authHeaders() });
    if (hr.ok) {
      document.getElementById('health-dot').className = 'status-dot success';
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
    list.innerHTML = '<p class="muted">No applications yet.</p>';
  } else {
    list.innerHTML = apps.map(a => `
      <button class="app-item" type="button" data-name="${esc(a.name)}" aria-pressed="false">
        <span class="status-dot ${a.status === 'running' ? 'success' : 'danger'}" aria-hidden="true"></span>
        <span>${esc(a.name)}</span><span class="metadata">${esc(a.status)}</span>
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
  if (logAbort) { logAbort.abort(); logAbort = null; }
  document.getElementById('empty-state').classList.remove('hidden');
  document.getElementById('detail').classList.add('hidden');
  document.querySelectorAll('#sidebar .app-item').forEach(el => el.classList.remove('active'));
  document.querySelectorAll('#sidebar .app-item').forEach(el => el.setAttribute('aria-pressed', 'false'));
}

function selectApp(name) {
  const app = apps.find(a => a.name === name);
  if (!app) return;
  if (selected === name && logAbort) return;
  selected = name;

  document.querySelectorAll('#sidebar .app-item').forEach(el => el.classList.remove('active'));
  const btn = document.querySelector(`#sidebar [data-name="${CSS.escape(name)}"]`);
  if (btn) { btn.classList.add('active'); btn.setAttribute('aria-pressed', 'true'); }

  document.getElementById('empty-state').classList.add('hidden');
  document.getElementById('detail').classList.remove('hidden');

  document.getElementById('config-panel').innerHTML = `
    <div class="row-between">
      <div>
        <h1>${esc(app.name)}</h1>
        <a href="http://${esc(app.hostname)}" target="_blank" rel="noreferrer" class="mono">${esc(app.hostname)} ↗</a>
      </div>
      <span class="badge ${app.status === 'running' ? 'success' : 'danger'}">
        <span class="status-dot" aria-hidden="true"></span>
        ${app.status}
      </span>
    </div>
    <div class="stack">
      <div>
        <p class="section-label">Image</p><div class="value mono">${esc(app.image)}</div>
      </div>
      <div>
        <p class="section-label">Container</p><p class="value mono">self-host-app-${esc(app.name)}</p>
      </div>
      <div class="details-grid">
        <div><p class="section-label">Hostname</p><p class="value mono">${esc(app.hostname)}</p></div>
        <div><p class="section-label">Status</p><p class="value mono">${esc(app.status)}</p></div>
      </div>
    </div>
    <p><button id="remove-btn" type="button" class="button button-danger">Remove application</button></p>
  `;

  document.getElementById('remove-btn').addEventListener('click', () => removeApp(app.name));

  document.getElementById('logs-panel').innerHTML = `
    <p class="section-label mono">Logs — ${esc(app.name)}</p>
    <div id="logs-content" class="mono" role="log" aria-live="polite"></div>
  `;

  startLogStream(app.name);
}

// ── Log streaming (fetch + ReadableStream) ──────────────────────
async function startLogStream(name) {
  if (logAbort) { logAbort.abort(); logAbort = null; }

  const abort = new AbortController();
  logAbort = abort;

  const contentEl = document.getElementById('logs-content');
  if (!contentEl) return;

  contentEl.innerHTML = '';

  try {
    const res = await fetch('/apps/' + encodeURIComponent(name) + '/logs', {
      headers: authHeaders(),
      signal: abort.signal,
    });

    if (!res.ok) {
      contentEl.innerHTML = '<p class="danger">Could not connect to the log stream.</p>';
      return;
    }

    const reader = res.body.getReader();
    const decoder = new TextDecoder();
    let buffer = '';

    while (true) {
      const { done, value } = await reader.read();
      if (done) break;

      buffer += decoder.decode(value, { stream: true });
      const lines = buffer.split('\n');
      buffer = lines.pop();

      for (const line of lines) {
        if (line === '') continue;
        // SSE format: "data: <content>"
        const data = line.startsWith('data: ') ? line.slice(6) : line;
        if (data === 'keepalive' || data === '') continue;
        const p = document.createElement('p');
        p.textContent = data;
        contentEl.appendChild(p);
      }

      const panel = document.getElementById('logs-panel');
      if (panel) panel.scrollTop = panel.scrollHeight;
    }
  } catch (err) {
    if (err.name !== 'AbortError') {
      const p = document.createElement('p');
      p.className = 'danger';
      p.textContent = 'Connection lost';
      contentEl.appendChild(p);
    }
  } finally {
    if (logAbort === abort) logAbort = null;
  }
}

// ── Deploy ──────────────────────────────────────────────────────
function showDeployModal() {
  document.querySelector('.app-shell').inert = true;
  document.getElementById('deploy-modal').classList.remove('hidden');
  document.getElementById('deploy-name').focus();
  document.getElementById('deploy-error').classList.add('hidden');
}

function closeDeployModal() {
  document.getElementById('deploy-modal').classList.add('hidden');
  document.querySelector('.app-shell').inert = false;
  document.getElementById('deploy-btn').focus();
}

function handleDialogKeydown(event) {
  const modal = document.getElementById('deploy-modal');
  if (modal.classList.contains('hidden')) return;
  if (event.key === 'Escape') { closeDeployModal(); return; }
  if (event.key !== 'Tab') return;
  const controls = [...modal.querySelectorAll('button, input')].filter(el => !el.disabled);
  const first = controls[0];
  const last = controls[controls.length - 1];
  if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
  if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
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
