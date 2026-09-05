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
let system = [];
let selected = null;
let selectedSystem = null;
let logAbort = null;
let pendingPoll = null;
let detailSignature = null;

async function init() {
  await loadBootstrap();
  await loadApps();
  await loadSystem();
  setupSplitter();
  document.getElementById('deploy-btn').addEventListener('click', showDeployModal);
  document.getElementById('empty-deploy').addEventListener('click', showDeployModal);
  document.querySelector('[data-home]').addEventListener('click', showDashboard);
  document.querySelector('[data-platform]').addEventListener('click', showPlatform);
  document.getElementById('health-link').addEventListener('click', showPlatform);
  document.getElementById('deploy-close').addEventListener('click', closeDeployModal);
  document.getElementById('deploy-cancel').addEventListener('click', closeDeployModal);
  document.getElementById('deploy-form').addEventListener('submit', deploy);
  document.querySelectorAll('input[name="deploy-source"]').forEach(radio => {
    radio.addEventListener('change', () => showDeploySource(radio.value));
  });
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
      document.getElementById('health-dot').className = 'dot success';
      document.getElementById('health-text').textContent = 'Healthy';
    }
  } catch {}
}

async function loadApps() {
  const before = Object.fromEntries(apps.map(a => [a.id, a.status]));
  try {
    const res = await fetch('/apps', { headers: authHeaders() });
    if (!res.ok) return;
    apps = await res.json();
  } catch { apps = []; }
  announceSettled(before);
  render();
  schedulePendingPoll();
}

async function loadSystem() {
  try {
    const res = await fetch('/system', { headers: authHeaders() });
    if (!res.ok) { system = []; return; }
    system = await res.json();
  } catch { system = []; }
}

/* A deploy is accepted before Docker starts pulling, so the row arrives as
   pending and settles minutes later. Keep asking while anything is in flight. */
function schedulePendingPoll() {
  if (pendingPoll) { clearTimeout(pendingPoll); pendingPoll = null; }
  if (!apps.some(a => a.status === 'pending')) return;
  pendingPoll = setTimeout(() => { pendingPoll = null; loadApps(); }, 2000);
}

/* The dialog is long gone by the time a pull finishes, so the outcome has to
   find the operator wherever they are. */
function announceSettled(before) {
  for (const app of apps) {
    if (before[app.id] !== 'pending' || app.status === 'pending') continue;
    if (app.status === 'running') {
      toast('success', 'Application deployed', app.name + ' is reachable at ' + app.hostname + '.');
    } else {
      toast('danger', 'Could not deploy ' + app.name, app.last_error || 'The deploy failed.');
    }
  }
}

// ── Render ──────────────────────────────────────────────────────
function render() {
  const list = document.getElementById('sidebar');
  const label = '<p class="t-caps">Applications</p>';
  if (apps.length === 0) {
    list.innerHTML = label + '<p class="nav-item muted">None yet</p>';
  } else {
    list.innerHTML = label + apps.map(a => `
      <button class="nav-item" type="button" data-id="${esc(a.id)}">
        <span class="dot ${statusTone(a.status) || 'subtle'}" aria-hidden="true"></span>
        <span class="grow truncate">${esc(a.name)}</span>
      </button>
    `).join('');

    list.querySelectorAll('[data-id]').forEach(btn => {
      btn.addEventListener('click', () => selectApp(btn.dataset.id));
    });
  }

  renderDashboard();

  if (selected) {
    const app = apps.find(a => a.id === selected);
    if (app) {
      // Redrawing on every poll would wipe out whatever is being typed into
      // the edit form, so only redraw when the record actually moved.
      if (appSignature(app) !== detailSignature) selectApp(selected);
    } else {
      selected = null;
      showDashboard();
    }
  } else if (selectedSystem) {
    // The Platform detail is read-only and nothing polls it, so leave it alone.
  } else if (!document.getElementById('platform').classList.contains('hidden')) {
    showPlatform();
  } else {
    showDashboard();
  }
}

/* running is the only good outcome; pending and failed both need attention,
   but only failed is an error. */
function appSignature(app) {
  return [app.status, app.name, app.image, app.hostname,
          hostnames(app).join(','), JSON.stringify(app.last_error || null),
          app.compose || '', app.web_service || '', app.web_port || '',
          (app.services || []).map(s => s.service + ':' + s.state).join(',')].join('|');
}

function isCompose(app) { return app.source === 'compose'; }

/* Every Hostname the application answers on: its Hostname, then its aliases. */
function hostnames(app) {
  return [app.hostname, ...(app.aliases || [])];
}

function statusTone(status) {
  if (status === 'running') return 'success';
  if (status === 'failed') return 'danger';
  return '';
}

/* Docker's word for a container, in the Application's vocabulary. */
function serviceTone(state) {
  if (state === 'running') return 'success';
  if (state === 'exited' || state === 'dead' || state === 'restarting' || state === 'missing') return 'danger';
  return '';
}

/* A status badge carries its own surface, so the tone has to name a badge
   variant and not just a text colour. Pending is neutral: it is not good news
   yet, and it is not bad news either. */
function statusBadge(status) {
  const tone = statusTone(status);
  return tone ? 'badge-' + tone : 'badge-neutral';
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
    <div class="card"><div class="stat"><span class="stat-label">Applications</span><span class="stat-value">${apps.length}</span></div></div>
    <div class="card"><div class="stat"><span class="stat-label">Running</span><span class="stat-value">${running}</span></div></div>
    <div class="card"><div class="stat"><span class="stat-label">Failed</span><span class="stat-value${failed ? ' danger' : ''}">${failed}</span></div></div>
  `;

  const rows = document.getElementById('app-rows');
  rows.innerHTML = apps.map(a => `
    <tr data-id="${esc(a.id)}" tabindex="0">
      <td>${esc(a.name)}</td>
      <td class="mono">${esc(a.hostname)}${(a.aliases || []).length ? `<span class="t-metadata muted"> +${(a.aliases || []).length}</span>` : ''}</td>
      <td class="mono">${esc(a.image)}</td>
      <td><span class="badge ${statusBadge(a.status)}"><span class="dot" aria-hidden="true"></span>${esc(a.status)}</span></td>
      <td class="num">${a.restarts === undefined ? '<span class="muted">—</span>' : a.restarts}</td>
    </tr>
  `).join('');

  document.getElementById('app-count').textContent =
    apps.length + (apps.length === 1 ? ' application' : ' applications');
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
  selectedSystem = null;
  detailSignature = null;
  setCrumb('Overview');
  document.querySelector('[data-home]').setAttribute('aria-current', 'page');
  document.querySelector('[data-platform]').removeAttribute('aria-current');
  document.getElementById('dashboard').classList.remove('hidden');
  document.getElementById('platform').classList.add('hidden');
  document.getElementById('detail').classList.add('hidden');
  document.querySelectorAll('#sidebar [data-id]').forEach(el => {
    el.removeAttribute('aria-current');
  });
}

function renderPlatform() {
  const rows = document.getElementById('system-rows');
  if (!rows) return;
  rows.innerHTML = system.map(c => `
    <tr data-role="${esc(c.role)}" tabindex="0">
      <td class="mono">${esc(c.role)}</td>
      <td class="mono">${esc(c.name)}</td>
      <td class="mono">${esc(c.image)}</td>
      <td><span class="badge ${statusBadge(c.status)}"><span class="dot" aria-hidden="true"></span>${esc(c.status)}</span></td>
      <td class="num">${c.restarts === null || c.restarts === undefined ? '<span class="muted">—</span>' : c.restarts}</td>
    </tr>
  `).join('');
  document.getElementById('system-count').textContent =
    system.length + (system.length === 1 ? ' Platform Infra component' : ' Platform Infra components');
  rows.querySelectorAll('tr[data-role]').forEach(tr => {
    tr.addEventListener('click', () => selectSystem(tr.dataset.role));
    tr.addEventListener('keydown', e => {
      if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); selectSystem(tr.dataset.role); }
    });
  });
}

function showPlatform() {
  if (logAbort) { logAbort.abort(); logAbort = null; }
  selected = null;
  selectedSystem = null;
  detailSignature = null;
  setCrumb('Platform');
  document.querySelector('[data-platform]').setAttribute('aria-current', 'page');
  document.querySelector('[data-home]').removeAttribute('aria-current');
  document.getElementById('platform').classList.remove('hidden');
  document.getElementById('dashboard').classList.add('hidden');
  document.getElementById('detail').classList.add('hidden');
  document.querySelectorAll('#sidebar [data-id]').forEach(el => {
    el.removeAttribute('aria-current');
  });
  renderPlatform();
}

/* The Platform Infra detail is the read-only twin of the Application detail:
   the same split, with the edit form replaced by a .kv definition list. No
   Remove, no Save — the absence of the actions is what says read-only. */
function selectSystem(role) {
  const c = system.find(x => x.role === role);
  if (!c) return;
  selected = null;
  selectedSystem = role;
  detailSignature = null;

  setCrumb('Platform');
  document.querySelector('[data-platform]').setAttribute('aria-current', 'page');
  document.querySelector('[data-home]').removeAttribute('aria-current');
  document.getElementById('dashboard').classList.add('hidden');
  document.getElementById('platform').classList.add('hidden');
  document.getElementById('detail').classList.remove('hidden');
  document.querySelectorAll('#sidebar [data-id]').forEach(el => {
    el.removeAttribute('aria-current');
  });

  const restarts = c.restarts === null || c.restarts === undefined ? '—' : String(c.restarts);

  document.getElementById('detail').innerHTML = `
    <div class="between">
      <div class="page-header">
        <h1 class="t-h1">${esc(c.role)}</h1>
        <p class="muted t-label"><span class="mono">${esc(c.name)}</span></p>
      </div>
      <span class="badge ${statusBadge(c.status)}">
        <span class="dot" aria-hidden="true"></span>${esc(c.status)}
      </span>
    </div>
    <div class="card detail-panel">
      <div class="split">
        <div class="pane">
          <p class="t-caps">Configuration</p>
          <dl class="kv">
            <dt>Role</dt><dd>${esc(c.role)}</dd>
            <dt>Container</dt><dd>${esc(c.name)}</dd>
            <dt>Image</dt><dd>${esc(c.image)}</dd>
            <dt>Restarts</dt><dd>${esc(restarts)}</dd>
          </dl>
        </div>
        <div class="splitter" id="splitter" aria-hidden="true"></div>
        <div class="pane pane-logs" id="logs-panel"></div>
      </div>
    </div>
  `;

  document.getElementById('logs-panel').innerHTML = `
    <p class="t-caps">Logs from ${esc(c.name)}</p>
    <div class="logview"><div class="log-scroll" id="logs-content" role="log" aria-live="polite"></div></div>
  `;

  startSystemLogStream(role);
}

function selectApp(id) {
  const app = apps.find(a => a.id === id);
  if (!app) return;
  selected = id;
  selectedSystem = null;
  detailSignature = appSignature(app);

  document.querySelectorAll('#sidebar [data-id]').forEach(el => {
    if (el.dataset.id === id) el.setAttribute('aria-current', 'page');
    else el.removeAttribute('aria-current');
  });

  setCrumb(app.name);
  document.querySelector('[data-home]').removeAttribute('aria-current');
  document.querySelector('[data-platform]').removeAttribute('aria-current');
  document.getElementById('dashboard').classList.add('hidden');
  document.getElementById('platform').classList.add('hidden');
  document.getElementById('detail').classList.remove('hidden');

  // The whole detail is one page: the identity is the page header, and the
  // split below it is only the two panes. Putting the header inside the
  // config pane made the title half a window wide and scroll with the form.
  const compose = isCompose(app);
  const canStop = app.status === 'running' || app.status === 'failed';
  const canStart = app.status === 'stopped' || app.status === 'failed';

  // A Compose Application is edited as its file; an image Application as its
  // image. Both route somewhere, but only Compose has a choice of where.
  const definitionFields = compose ? `
            <div class="field">
              <label for="edit-compose">Compose file</label>
              <textarea class="textarea mono compose-editor" id="edit-compose" rows="14" spellcheck="false"
                        aria-describedby="edit-compose-help edit-compose-error">${esc(app.compose || '')}</textarea>
              <small class="field-hint" id="edit-compose-help">Saving brings the project up again; only services whose definition changed are recreated.</small>
              <small class="field-error hidden" id="edit-compose-error"></small>
            </div>
            <div class="field-row">
              <div class="field">
                <label for="edit-web-service">Web service</label>
                <input class="input mono" type="text" id="edit-web-service" value="${esc(app.web_service || '')}">
              </div>
              <div class="field">
                <label for="edit-web-port">Web port</label>
                <input class="input mono" type="number" id="edit-web-port" min="1" max="65535" value="${app.web_port || ''}">
              </div>
            </div>` : `
            <div class="field">
              <label for="edit-image">Image</label>
              <input class="input mono" type="text" id="edit-image" value="${esc(app.image)}" required>
            </div>`;

  document.getElementById('detail').innerHTML = `
    <div class="between">
      <div class="page-header">
        <h1 class="t-h1">${esc(app.name)}</h1>
        <p class="muted t-label">${hostnames(app).map(h =>
          `<a href="https://${esc(h)}" target="_blank" rel="noreferrer" class="mono">${esc(h)}</a>`
        ).join(' · ')}</p>
      </div>
      <div class="lifecycle">
        <span class="badge ${statusBadge(app.status)}">
          <span class="dot" aria-hidden="true"></span>${esc(app.status)}
        </span>
        <button type="button" class="btn btn-outline btn-sm" data-lifecycle="start" ${canStart ? '' : 'disabled'}>Start</button>
        <button type="button" class="btn btn-outline btn-sm" data-lifecycle="stop" ${canStop ? '' : 'disabled'}>Stop</button>
        <button type="button" class="btn btn-outline btn-sm" data-lifecycle="restart" ${app.status === 'running' ? '' : 'disabled'}>Restart</button>
      </div>
    </div>
    ${app.last_error ? `<div class="alert alert-danger" role="alert">${alertMarkup(app.last_error, 'Retry')}</div>` : ''}
    <div id="edit-error" class="hidden"></div>
    <div class="card detail-panel">
      <div class="split">
        <div class="pane">
          <form class="stack" id="edit-form">
            <p class="t-caps">Configuration</p>
            <div class="field">
              <label for="edit-name">Name</label>
              <input class="input" type="text" id="edit-name" value="${esc(app.name)}" required>
            </div>
            ${definitionFields}
            <div class="field">
              <label for="edit-hostname">Hostname</label>
              <input class="input mono" type="text" id="edit-hostname" value="${esc(app.hostname)}" required
                     aria-describedby="edit-hostname-help edit-hostname-error">
              <small class="field-hint" id="edit-hostname-help">Takes effect immediately; the container keeps running.</small>
              <small class="field-error hidden" id="edit-hostname-error"></small>
            </div>
            <div class="field">
              <label for="edit-aliases">Aliases</label>
              <input class="input mono" type="text" id="edit-aliases" value="${esc((app.aliases || []).join(', '))}"
                     placeholder="old-name.${esc(dnsSuffix())}" aria-describedby="edit-aliases-help edit-aliases-error">
              <small class="field-hint" id="edit-aliases-help">Other hostnames this application also answers on, comma separated. Keep the old one here to change the Hostname without breaking it.</small>
              <small class="field-error hidden" id="edit-aliases-error"></small>
            </div>
            ${servicesMarkup(app)}
            <div class="form-actions">
              <button id="remove-btn" type="button" class="btn btn-danger-ghost btn-sm">Remove application</button>
              <button type="submit" class="btn btn-primary btn-sm">Save and redeploy</button>
            </div>
          </form>
        </div>
        <div class="splitter" id="splitter" aria-hidden="true"></div>
        <div class="pane pane-logs" id="logs-panel"></div>
      </div>
    </div>
  `;

  document.getElementById('remove-btn').addEventListener('click', () => removeApp(app));
  document.getElementById('edit-form').addEventListener('submit', e => saveApp(e, app.id));
  document.querySelectorAll('[data-lifecycle]').forEach(btn => {
    btn.addEventListener('click', () => lifecycle(app, btn.dataset.lifecycle));
  });
  const retry = document.querySelector('[data-retry]');
  if (retry) retry.addEventListener('click', () => {
    document.getElementById('edit-form').requestSubmit();
  });

  // Rendering the detail replaced the log pane, so the stream always restarts:
  // the element the running one holds is no longer in the document. This also
  // keeps a redeploy honest — it replaces the container, and output from the
  // previous one must not keep scrolling past.
  document.getElementById('logs-panel').innerHTML = `
    <p class="t-caps">Logs from sf-app-${esc(app.id)}${compose ? '-*' : ''}</p>
    <div class="logview"><div class="log-scroll" id="logs-content" role="log" aria-live="polite"></div></div>
  `;

  startLogStream(app.id);
}

/* Every container of the Application and what Docker says about it. For a
   one-container Application this is one row; it is still the row that says
   "exited with code 1" when the deploy went fine and the process did not. */
function servicesMarkup(app) {
  const services = app.services || [];
  if (services.length === 0) return '';
  return `
    <div class="services">
      <p class="t-caps">Services</p>
      <div class="table-wrap"><div class="table-scroll"><table class="table">
        <thead><tr><th scope="col">Service</th><th scope="col">Container</th><th scope="col">State</th><th scope="col" class="num">Restarts</th></tr></thead>
        <tbody>${services.map(s => `
          <tr>
            <td class="mono">${esc(s.service)}</td>
            <td class="mono">${esc(s.container)}</td>
            <td><span class="badge ${serviceTone(s.state) ? 'badge-' + serviceTone(s.state) : 'badge-neutral'}"><span class="dot" aria-hidden="true"></span>${esc(s.state)}${s.state === 'exited' && s.exit_code !== undefined ? ' (' + s.exit_code + ')' : ''}</span></td>
            <td class="num">${s.restarts === undefined ? '<span class="muted">—</span>' : s.restarts}</td>
          </tr>`).join('')}
        </tbody>
      </table></div></div>
    </div>`;
}

// ── Lifecycle ───────────────────────────────────────────────────
async function lifecycle(app, verb) {
  try {
    const res = await fetch('/apps/id/' + encodeURIComponent(app.id) + '/' + verb, {
      method: 'POST',
      headers: authHeaders(),
    });
    if (!res.ok) {
      toast('danger', 'Could not ' + verb + ' ' + app.name, await failureOf(res));
    } else {
      const updated = await res.json();
      toast(updated.status === 'failed' ? 'danger' : 'success',
            app.name + ' is ' + updated.status,
            updated.status === 'failed' ? updated.last_error : null);
    }
  } catch (err) {
    toast('danger', 'Could not ' + verb + ' ' + app.name, err.message);
  }
  await loadApps();
}

// ── Log streaming (fetch + ReadableStream) ──────────────────────
async function startLogStream(id) {
  await openLogStream('/apps/id/' + encodeURIComponent(id) + '/logs');
}

async function startSystemLogStream(role) {
  await openLogStream('/system/' + encodeURIComponent(role) + '/logs');
}

async function openLogStream(url) {
  if (logAbort) { logAbort.abort(); logAbort = null; }

  const abort = new AbortController();
  logAbort = abort;

  const contentEl = document.getElementById('logs-content');
  if (!contentEl) return;

  contentEl.innerHTML = '';

  try {
    const res = await fetch(url, {
      headers: authHeaders(),
      signal: abort.signal,
    });

    if (!res.ok) {
      contentEl.innerHTML = '<div class="log-error">Could not connect to the log stream.</div>';
      return;
    }

    const reader = res.body.getReader();
    const decoder = new TextDecoder();
    let buffer = '';
    let currentEvent = 'message';

    while (true) {
      const { done, value } = await reader.read();
      if (done) break;

      buffer += decoder.decode(value, { stream: true });
      const lines = buffer.split('\n');
      buffer = lines.pop();

      for (const line of lines) {
        // SSE format: "event: <name>" followed by "data: <content>".
        if (line.startsWith('event: ')) { currentEvent = line.slice(7); continue; }
        if (line === '') continue;
        const data = line.startsWith('data: ') ? line.slice(6) : line;
        if (data === 'keepalive' || data === '') continue;
        const el = document.createElement('div');
        el.textContent = data;
        if (currentEvent === 'notice') el.className = 'log-info';
        contentEl.appendChild(el);
        currentEvent = 'message';
      }

      contentEl.scrollTop = contentEl.scrollHeight;
    }
  } catch (err) {
    if (err.name !== 'AbortError') {
      const el = document.createElement('div');
      el.className = 'log-error';
      el.textContent = 'Connection lost';
      contentEl.appendChild(el);
    }
  } finally {
    if (logAbort === abort) logAbort = null;
  }
}

// ── Deploy ──────────────────────────────────────────────────────
function showDeployModal() {
  document.querySelector('.app-shell').inert = true;
  document.getElementById('deploy-modal').classList.remove('hidden');
  document.getElementById('deploy-hostname').placeholder = 'my-app.' + dnsSuffix();
  document.getElementById('deploy-aliases').placeholder = 'old-name.' + dnsSuffix();
  document.getElementById('deploy-name').focus();
  document.getElementById('deploy-error').classList.add('hidden');
  clearFieldError('deploy-hostname');
  clearFieldError('deploy-aliases');
  clearFieldError('deploy-compose');
  showDeploySource(deploySource());
}

function deploySource() {
  return document.querySelector('input[name="deploy-source"]:checked').value;
}

/* The two definitions share one dialog. Only the visible one is required,
   or the browser would refuse to submit over a field the operator cannot
   see. */
function showDeploySource(source) {
  document.querySelectorAll('#deploy-modal [data-source]').forEach(el => {
    el.classList.toggle('hidden', el.dataset.source !== source);
  });
  document.getElementById('deploy-image').required = source === 'image';
  document.getElementById('deploy-compose').required = source === 'compose';
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
  const controls = [...modal.querySelectorAll('button, input, textarea')].filter(el => !el.disabled && el.offsetParent !== null);
  const first = controls[0];
  const last = controls[controls.length - 1];
  if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
  if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
}

/* A comma or a space both read as "and another one" to someone typing a list,
   so accept either rather than rejecting the one that was not asked for. */
function parseAliases(value) {
  return value.split(/[,\s]+/).map(a => a.trim()).filter(Boolean);
}

function dnsSuffix() {
  return document.getElementById('dns-suffix').textContent;
}

function clearFieldError(inputId) {
  const input = document.getElementById(inputId);
  const error = document.getElementById(inputId + '-error');
  input.removeAttribute('aria-invalid');
  error.textContent = '';
  error.classList.add('hidden');
}

function showFieldError(inputId, failure) {
  const input = document.getElementById(inputId);
  const error = document.getElementById(inputId + '-error');
  input.setAttribute('aria-invalid', 'true');
  error.textContent = asReport(failure).error;
  error.classList.remove('hidden');
  input.focus();
}

function routingErrorField(prefix, failure, aliases) {
  const message = asReport(failure).error;
  const conflictingAlias = aliases.some(alias => message.includes("'" + alias + "'"));
  return prefix + (conflictingAlias ? '-aliases' : '-hostname');
}

async function saveApp(e, id) {
  e.preventDefault();
  document.getElementById('edit-error').className = 'hidden';
  clearFieldError('edit-hostname');
  clearFieldError('edit-aliases');

  const aliases = parseAliases(document.getElementById('edit-aliases').value);
  const body = {
    name: document.getElementById('edit-name').value.trim(),
    hostname: document.getElementById('edit-hostname').value.trim(),
    aliases,
  };
  const composeEl = document.getElementById('edit-compose');
  if (composeEl) {
    clearFieldError('edit-compose');
    body.compose = composeEl.value;
    body.web_service = document.getElementById('edit-web-service').value.trim();
    const port = document.getElementById('edit-web-port').value.trim();
    if (port) body.web_port = Number(port);
  } else {
    body.image = document.getElementById('edit-image').value.trim();
  }

  let failure = null;
  try {
    const res = await fetch('/apps/id/' + encodeURIComponent(id), {
      method: 'PUT',
      headers: authHeaders(),
      body: JSON.stringify(body),
    });
    if (res.status === 202) {
      toast('success', 'Changes saved', body.name + ' is redeploying.');
    } else if (res.ok) {
      toast('success', 'Changes saved', body.name + ' was updated.');
    } else {
      failure = await failureOf(res);
    }
  } catch (err) {
    failure = { error: 'Network error', caused_by: [err.message] };
  }

  // Reload either way: a failed redeploy still changed the stored record.
  await loadApps();

  if (!failure) return;

  if (failure.error.startsWith('invalid Application Hostname:')) {
    showFieldError(routingErrorField('edit', failure, aliases), failure);
    return;
  }
  if (failure.error.startsWith('invalid Compose definition') && document.getElementById('edit-compose')) {
    showFieldError('edit-compose', { error: failure.caused_by[0] || failure.error, caused_by: [] });
    return;
  }

  // The record already carries the reason when the deploy itself failed, and
  // selectApp rendered it. Only errors that never reached the database — a
  // rejected name, a clash — need saying here.
  const saved = apps.find(a => a.id === id);
  if (saved && saved.last_error) return;
  showAlert(document.getElementById('edit-error'), failure);
}

async function deploy(e) {
  e.preventDefault();
  const name = document.getElementById('deploy-name').value.trim();
  const source = deploySource();
  const image = source === 'image' ? document.getElementById('deploy-image').value.trim() : '';
  const hostname = document.getElementById('deploy-hostname').value.trim();
  const aliases = parseAliases(document.getElementById('deploy-aliases').value);
  const errEl = document.getElementById('deploy-error');
  errEl.classList.add('hidden');
  clearFieldError('deploy-hostname');
  clearFieldError('deploy-aliases');
  clearFieldError('deploy-compose');

  const body = { name, image, aliases };
  if (hostname) body.hostname = hostname;
  if (source === 'compose') {
    body.compose = document.getElementById('deploy-compose').value;
    const service = document.getElementById('deploy-web-service').value.trim();
    const port = document.getElementById('deploy-web-port').value.trim();
    if (service) body.web_service = service;
    if (port) body.web_port = Number(port);
  }

  try {
    const res = await fetch('/apps', {
      method: 'POST',
      headers: authHeaders(),
      body: JSON.stringify(body),
    });
    const failure = res.ok ? null : await failureOf(res);

    await loadApps();
    const created = apps.find(a => a.name === name);

    // A rejected name never reached the database. Keep the dialog open with
    // what was typed, so the fix is one edit rather than a retype.
    if (failure && !created) {
      if (failure.error.startsWith('invalid Application Hostname:')) {
        showFieldError(routingErrorField('deploy', failure, aliases), failure);
        return;
      }
      if (failure.error.startsWith('invalid Compose definition')) {
        showFieldError('deploy-compose', { error: failure.caused_by[0] || failure.error, caused_by: [] });
        return;
      }
      showAlert(errEl, failure);
      return;
    }

    document.getElementById('deploy-name').value = '';
    document.getElementById('deploy-image').value = '';
    document.getElementById('deploy-compose').value = '';
    document.getElementById('deploy-web-service').value = '';
    document.getElementById('deploy-web-port').value = '';
    document.getElementById('deploy-hostname').value = '';
    document.getElementById('deploy-aliases').value = '';
    closeDeployModal();

    // The Application is on record even when the deploy failed, so open it
    // instead of reporting an error the operator cannot act on.
    if (created) selectApp(created.id);

    if (!failure) {
      // Accepted, not finished: the pull runs on the platform and the poll
      // says how it went.
      toast('success', 'Deploy started for ' + name,
            source === 'compose' ? 'Bringing the Compose project up ...' : 'Pulling image ' + image + ' ...');
    }
    // A failure needs no toast: selectApp already rendered the reason as the
    // application's alert, and that one stays on screen.
  } catch (err) {
    showAlert(errEl, { error: 'Could not reach the platform', caused_by: [err.message] });
  }
}

// ── Remove ──────────────────────────────────────────────────────
async function removeApp(app) {
  const name = app.name;
  if (!await confirmRemove(app)) return;
  try {
    const res = await fetch('/apps/' + encodeURIComponent(name), {
      method: 'DELETE',
      headers: authHeaders(),
    });
    if (!res.ok) {
      toast('danger', 'Could not remove ' + name, await failureOf(res));
      return;
    }
    toast('success', 'Application removed', name + ' and its containers are gone. Its data stays on the Host.');
  } catch (err) {
    toast('danger', 'Could not remove ' + name, err.message);
    return;
  }
  selected = null;
  await loadApps();
}

// ── Alerts ──────────────────────────────────────────────────────
const ALERT_ICON = '<svg class="icon" aria-hidden="true"><use href="#i-alert"/></svg>';

/* Every failure reaches the console as { error, caused_by }: the API answers
   that shape and a stored last_error keeps it. Anything else — a network
   error, a body that is not JSON — is a failure with nothing underneath. */
function asReport(value) {
  if (value && typeof value === 'object' && typeof value.error === 'string') {
    return { error: value.error, caused_by: value.caused_by || [] };
  }
  return { error: String(value || 'Something went wrong.'), caused_by: [] };
}

/* Reads a failed response as a report. A body that is not JSON — a proxy
   error, an empty 502 — still says something, so it becomes the failure. */
async function failureOf(res) {
  const body = await res.text();
  try {
    return asReport(JSON.parse(body));
  } catch {
    return asReport(body.trim() || res.statusText);
  }
}

/* One layer per line, in the order the platform stacked them. Naming the
   layers is the whole point: the reader can tell the Platform's framing apart
   from what Docker said. */
function causesMarkup(report) {
  if (!report.caused_by.length) return '';
  return `<ol class="alert-causes">${report.caused_by.map(c => `<li>${esc(c)}</li>`).join('')}</ol>`;
}

/* The single place an error gets rendered. Title says what failed, body says
   why, and the action is the way out — the same shape every time. */
function alertMarkup(failure, actionLabel) {
  const report = asReport(failure);
  return `
    ${ALERT_ICON}
    <div class="grow">
      <p class="alert-title">${esc(report.error)}</p>
      ${causesMarkup(report)}
    </div>
    ${actionLabel ? `<button type="button" class="btn btn-outline btn-sm" data-retry>${esc(actionLabel)}</button>` : ''}
  `;
}

function showAlert(el, failure, actionLabel) {
  if (!el) return;
  el.className = 'alert alert-danger';
  el.setAttribute('role', 'alert');
  el.innerHTML = alertMarkup(failure, actionLabel);
}

// ── Toasts and confirmation ─────────────────────────────────────
const TOAST_ICONS = {
  success: '<svg class="icon success" aria-hidden="true"><use href="#i-check"/></svg>',
  danger: '<svg class="icon danger" aria-hidden="true"><use href="#i-alert"/></svg>',
};

/* A toast shows up detached from whatever raised it, so the title has to name
   the Application. The failure and its causes go in the body, under it. */
function toast(kind, title, body) {
  const region = document.querySelector('[data-toasts]');
  if (!region) return;

  const report = body ? asReport(body) : null;
  const el = document.createElement('div');
  el.className = 'toast';
  el.innerHTML = `
    ${TOAST_ICONS[kind] || ''}
    <div class="grow">
      <p class="alert-title">${esc(title)}</p>
      ${report ? `<p class="alert-body">${esc(report.error)}</p>${causesMarkup(report)}` : ''}
    </div>
    <button type="button" class="btn btn-ghost btn-icon btn-sm" aria-label="Dismiss"><svg class="icon icon-sm" aria-hidden="true"><use href="#i-x"/></svg></button>
  `;
  const dismiss = () => el.remove();
  el.querySelector('button').addEventListener('click', dismiss);
  region.appendChild(el);
  setTimeout(dismiss, 6000);
}

/* Replaces window.confirm, which cannot be styled and says the hostname out
   loud. Resolves true only if the destructive button is the one pressed. */
function confirmRemove(app) {
  return new Promise(resolve => {
    const modal = document.getElementById('confirm-modal');
    const accept = document.getElementById('confirm-accept');
    const cancel = document.getElementById('confirm-cancel');
    document.getElementById('confirm-body').textContent =
      `"${app.name}" and its ${isCompose(app) ? 'containers' : 'container'} will be removed. ` +
      'Named volumes and the data directory are kept on the Host.';

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
const MIN_PANE = 320;

/* The form pane never shrinks past a readable measure and the log pane keeps
   enough width for a line of output. Clamping in pixels rather than in
   percent is the point: 15% of a wide window is a usable form, 15% of a
   narrow one is a column of single words. */
function setupSplitter() {
  const detail = document.getElementById('detail');
  let dragging = null;

  detail.addEventListener('mousedown', (event) => {
    const splitter = event.target.closest('.splitter');
    if (!splitter) return;
    event.preventDefault();
    dragging = splitter.closest('.split');
    splitter.classList.add('dragging');
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';
  });

  document.addEventListener('mousemove', (event) => {
    if (!dragging) return;
    const [config, splitter] = dragging.children;
    const rect = dragging.getBoundingClientRect();
    const total = rect.width - splitter.offsetWidth;
    const min = MIN_PANE;
    const max = Math.max(min, total - MIN_PANE);
    config.style.flex = `0 0 ${Math.min(max, Math.max(min, event.clientX - rect.left))}px`;
  });

  document.addEventListener('mouseup', () => {
    if (!dragging) return;
    dragging.querySelector('.splitter').classList.remove('dragging');
    dragging = null;
    document.body.style.cursor = '';
    document.body.style.userSelect = '';
  });
}

// ── Utils ───────────────────────────────────────────────────────
function esc(s) { return s.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;'); }
