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
  document.getElementById('empty-deploy').addEventListener('click', showDeployModal);
  document.querySelector('[data-home]').addEventListener('click', showDashboard);
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
      <button class="app-item" type="button" data-id="${esc(a.id)}" aria-pressed="false">
        <span class="status-dot ${statusTone(a.status)}" aria-hidden="true"></span>
        <span class="grow">${esc(a.name)}</span><span class="metadata">${esc(a.status)}</span>
      </button>
    `).join('');

    list.querySelectorAll('.app-item').forEach(btn => {
      btn.addEventListener('click', () => selectApp(btn.dataset.id));
    });
  }

  renderDashboard();

  if (selected) {
    const app = apps.find(a => a.id === selected);
    if (app) {
      selectApp(selected);
    } else {
      selected = null;
      showDashboard();
    }
  } else {
    showDashboard();
  }
}

/* running is the only good outcome; pending and failed both need attention,
   but only failed is an error. */
function statusTone(status) {
  if (status === 'running') return 'success';
  if (status === 'failed') return 'danger';
  return '';
}

function renderDashboard() {
  const empty = apps.length === 0;
  document.getElementById('dashboard-empty').classList.toggle('hidden', !empty);
  document.getElementById('dashboard-content').classList.toggle('hidden', empty);

  const suffix = document.getElementById('dns-suffix').textContent;
  document.getElementById('dashboard-subtitle').textContent = empty
    ? 'Nothing deployed on ' + suffix + ' yet.'
    : apps.length + (apps.length === 1 ? ' application on ' : ' applications on ') + suffix;

  if (empty) return;

  const running = apps.filter(a => a.status === 'running').length;
  const failed = apps.filter(a => a.status === 'failed').length;

  document.getElementById('stat-grid').innerHTML = `
    <div class="card stat"><span class="section-label">Applications</span><span class="stat-value">${apps.length}</span></div>
    <div class="card stat"><span class="section-label">Running</span><span class="stat-value">${running}</span></div>
    <div class="card stat"><span class="section-label">Failed</span><span class="stat-value${failed ? ' danger' : ''}">${failed}</span></div>
  `;

  const rows = document.getElementById('app-rows');
  rows.innerHTML = apps.map(a => `
    <tr data-id="${esc(a.id)}" tabindex="0">
      <td>${esc(a.name)}</td>
      <td class="mono">${esc(a.image)}</td>
      <td class="mono">${esc(a.hostname)}</td>
      <td><span class="badge ${statusTone(a.status)}"><span class="status-dot" aria-hidden="true"></span>${esc(a.status)}</span></td>
    </tr>
  `).join('');
  rows.querySelectorAll('tr[data-id]').forEach(tr => {
    tr.addEventListener('click', () => selectApp(tr.dataset.id));
    tr.addEventListener('keydown', e => {
      if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); selectApp(tr.dataset.id); }
    });
  });
}

function setCrumb(label) {
  document.querySelector('.breadcrumb [aria-current]').textContent = label;
}

function showDashboard() {
  if (logAbort) { logAbort.abort(); logAbort = null; }
  selected = null;
  setCrumb('Overview');
  document.querySelector('[data-home]').setAttribute('aria-current', 'page');
  document.getElementById('dashboard').classList.remove('hidden');
  document.getElementById('detail').classList.add('hidden');
  document.querySelectorAll('#sidebar .app-item').forEach(el => {
    el.classList.remove('active');
    el.setAttribute('aria-pressed', 'false');
  });
}

function selectApp(id) {
  const app = apps.find(a => a.id === id);
  if (!app) return;
  const reselecting = selected === id && logAbort;
  selected = id;

  document.querySelectorAll('#sidebar .app-item').forEach(el => {
    const on = el.dataset.id === id;
    el.classList.toggle('active', on);
    el.setAttribute('aria-pressed', String(on));
  });

  setCrumb(app.name);
  document.querySelector('[data-home]').removeAttribute('aria-current');
  document.getElementById('dashboard').classList.add('hidden');
  document.getElementById('detail').classList.remove('hidden');

  document.getElementById('config-panel').innerHTML = `
    <div class="row-between page-header">
      <div>
        <button class="button button-ghost" type="button" data-back>← All applications</button>
        <h1>${esc(app.name)}</h1>
        <a href="http://${esc(app.hostname)}" target="_blank" rel="noreferrer" class="mono">${esc(app.hostname)} ↗</a>
      </div>
      <span class="badge ${statusTone(app.status)}">
        <span class="status-dot" aria-hidden="true"></span>
        ${esc(app.status)}
      </span>
    </div>
    ${app.last_error ? `<p class="alert" role="alert">${esc(app.last_error)}</p>` : ''}
    <form class="card stack" id="edit-form">
      <div class="field">
        <label for="edit-name">Name</label>
        <input class="input" type="text" id="edit-name" value="${esc(app.name)}" required>
      </div>
      <div class="field">
        <label for="edit-image">Image</label>
        <input class="input mono" type="text" id="edit-image" value="${esc(app.image)}" required>
      </div>
      <div class="field">
        <label for="edit-hostname">Hostname</label>
        <input class="input mono" type="text" id="edit-hostname" value="${esc(app.hostname)}" required>
        <small class="muted">Changing this restarts the container.</small>
      </div>
      <div class="field">
        <span class="section-label">Container</span>
        <p class="value mono">sf-app-${esc(app.id)}</p>
      </div>
      <p id="edit-error" class="alert hidden" role="alert"></p>
      <div class="dialog-actions">
        <button id="remove-btn" type="button" class="button button-danger">Remove application</button>
        <button type="submit" class="button button-primary">Save and redeploy</button>
      </div>
    </form>
  `;

  document.querySelector('[data-back]').addEventListener('click', showDashboard);
  document.getElementById('remove-btn').addEventListener('click', () => removeApp(app.name));
  document.getElementById('edit-form').addEventListener('submit', e => saveApp(e, app.id));

  if (reselecting) return;

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

async function saveApp(e, id) {
  e.preventDefault();
  document.getElementById('edit-error').classList.add('hidden');

  const body = {
    name: document.getElementById('edit-name').value.trim(),
    image: document.getElementById('edit-image').value.trim(),
    hostname: document.getElementById('edit-hostname').value.trim(),
  };

  let failure = null;
  try {
    const res = await fetch('/apps/id/' + encodeURIComponent(id), {
      method: 'PUT',
      headers: authHeaders(),
      body: JSON.stringify(body),
    });
    if (res.ok) {
      toast('success', 'Changes saved', body.name + ' was redeployed.');
    } else {
      failure = await res.text();
    }
  } catch (err) {
    failure = 'Network error: ' + err.message;
  }

  // Reload either way: a failed redeploy still changed the stored record.
  await loadApps();

  if (!failure) return;
  toast('danger', 'Could not save ' + body.name, failure);

  // Only write it on the form when the record does not already say it —
  // otherwise the same sentence appears twice, once above the form and once
  // inside it.
  const saved = apps.find(a => a.id === id);
  if (saved && saved.last_error) return;
  const errEl = document.getElementById('edit-error');
  if (errEl) {
    errEl.textContent = failure;
    errEl.classList.remove('hidden');
  }
}

async function deploy(e) {
  e.preventDefault();
  const name = document.getElementById('deploy-name').value.trim();
  const image = document.getElementById('deploy-image').value.trim();
  const errEl = document.getElementById('deploy-error');
  errEl.classList.add('hidden');

  try {
    const res = await fetch('/apps', {
      method: 'POST',
      headers: authHeaders(),
      body: JSON.stringify({ name, image }),
    });
    const failure = res.ok ? null : await res.text();

    await loadApps();
    const created = apps.find(a => a.name === name);

    // A rejected name never reached the database. Keep the dialog open with
    // what was typed, so the fix is one edit rather than a retype.
    if (failure && !created) {
      errEl.textContent = failure;
      errEl.classList.remove('hidden');
      return;
    }

    document.getElementById('deploy-name').value = '';
    document.getElementById('deploy-image').value = '';
    closeDeployModal();

    // The Application is on record even when the deploy failed, so open it
    // instead of reporting an error the operator cannot act on.
    if (created) selectApp(created.id);

    if (failure) {
      // One toast, and no inline copy: the record was saved with last_error,
      // so selectApp already rendered the same text above the form.
      toast('danger', 'Could not start ' + name, failure);
    } else {
      toast('success', 'Application deployed', created
        ? name + ' is reachable at ' + created.hostname + '.'
        : name + ' was created.');
    }
  } catch (err) {
    errEl.textContent = 'Network error: ' + err.message;
    errEl.classList.remove('hidden');
  }
}

// ── Remove ──────────────────────────────────────────────────────
async function removeApp(name) {
  if (!await confirmRemove(name)) return;
  try {
    const res = await fetch('/apps/' + encodeURIComponent(name), {
      method: 'DELETE',
      headers: authHeaders(),
    });
    if (!res.ok) {
      toast('danger', 'Could not remove ' + name, await res.text());
      return;
    }
    toast('success', 'Application removed', name + ' and its container are gone.');
  } catch (err) {
    toast('danger', 'Could not remove ' + name, err.message);
    return;
  }
  selected = null;
  await loadApps();
}

// ── Toasts and confirmation ─────────────────────────────────────
const TOAST_ICONS = {
  success: '<svg viewBox="0 0 24 24" class="success"><path d="M20 6 9 17l-5-5"></path></svg>',
  danger: '<svg viewBox="0 0 24 24" class="danger"><circle cx="12" cy="12" r="10"></circle><path d="M12 8v5M12 16h.01"></path></svg>',
};

function toast(kind, title, body) {
  const region = document.querySelector('[data-toasts]');
  if (!region) return;

  const el = document.createElement('div');
  el.className = 'toast';
  el.innerHTML = `
    ${TOAST_ICONS[kind] || ''}
    <div class="grow">
      <p class="alert-title">${esc(title)}</p>
      ${body ? `<p class="alert-body">${esc(body)}</p>` : ''}
    </div>
    <button type="button" class="button button-ghost" aria-label="Dismiss">✕</button>
  `;
  const dismiss = () => el.remove();
  el.querySelector('button').addEventListener('click', dismiss);
  region.appendChild(el);
  setTimeout(dismiss, 6000);
}

/* Replaces window.confirm, which cannot be styled and says the hostname out
   loud. Resolves true only if the destructive button is the one pressed. */
function confirmRemove(name) {
  return new Promise(resolve => {
    const modal = document.getElementById('confirm-modal');
    const accept = document.getElementById('confirm-accept');
    const cancel = document.getElementById('confirm-cancel');
    document.getElementById('confirm-body').textContent =
      `"${name}" and its container will be removed. This cannot be undone.`;

    const close = (answer) => {
      modal.classList.add('hidden');
      accept.removeEventListener('click', onAccept);
      cancel.removeEventListener('click', onCancel);
      modal.removeEventListener('click', onBackdrop);
      document.removeEventListener('keydown', onKey);
      resolve(answer);
    };
    const onAccept = () => close(true);
    const onCancel = () => close(false);
    const onBackdrop = (e) => { if (e.target === modal) close(false); };
    const onKey = (e) => { if (e.key === 'Escape') close(false); };

    accept.addEventListener('click', onAccept);
    cancel.addEventListener('click', onCancel);
    modal.addEventListener('click', onBackdrop);
    document.addEventListener('keydown', onKey);

    modal.classList.remove('hidden');
    accept.focus();
  });
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
