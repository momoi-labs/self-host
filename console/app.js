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
/* True while the "New application" form is on screen, so a poll does not
   redraw over what is being typed. */
let creating = false;
let logAbort = null;
let pendingPoll = null;
let detailSignature = null;
let historyReady = false;

async function init() {
  await loadBootstrap();
  await loadApps();
  await loadSystem();
  setupSplitter();
  document.getElementById('deploy-btn').addEventListener('click', showNewApp);
  document.getElementById('empty-deploy').addEventListener('click', showNewApp);
  document.querySelector('[data-home]').addEventListener('click', showDashboard);
  document.getElementById('service-search').addEventListener('input', renderDashboard);
  document.getElementById('show-platform').addEventListener('change', renderDashboard);
  document.getElementById('clear-filters').addEventListener('click', () => {
    document.getElementById('service-search').value = '';
    document.getElementById('show-platform').checked = false;
    renderDashboard();
    document.getElementById('service-search').focus();
  });
  document.getElementById('health-link').addEventListener('click', showPlatform);
  historyReady = true;
  restoreView();
  window.addEventListener('popstate', restoreView);
}

function rememberView(view, id = null) {
  if (!historyReady) return;
  const state = { view, id };
  if (!history.state) {
    history.replaceState(state, '');
  } else if (history.state.view !== view || history.state.id !== id) {
    history.pushState(state, '');
  }
}

function restoreView() {
  const state = history.state;
  if (state?.view === 'app' && apps.some(a => a.id === state.id)) {
    selectApp(state.id);
  } else if (state?.view === 'system' && system.some(c => c.role === state.id)) {
    selectSystem(state.id);
  } else if (state?.view === 'new') {
    showNewApp();
  } else {
    history.replaceState({ view: 'overview', id: null }, '');
    showDashboard();
  }
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
    if (!res.ok) { system = []; renderDashboard(); return; }
    system = await res.json();
  } catch { system = []; }
  renderDashboard();
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
/* Rebuilding a list throws away the element the keyboard is standing on, and
   focus falls back to the top of the document. While an Application is
   deploying the list is rebuilt every two seconds, which is exactly when the
   Operator is trying to reach it: without this, tabbing to an Application and
   pressing Enter is a race against the poll.

   Focus is remembered by what the element points at, not by the node, and
   restored onto whatever new node means the same thing. */
function preservingFocus(redraw) {
  const active = document.activeElement;
  const scope = active && active.closest
    ? active.closest('#sidebar, #app-rows, #system-rows, #detail')
    : null;
  const mark = scope && {
    scope: scope.id,
    id: active.id || null,
    appId: active.dataset.id || null,
    role: active.dataset.role || null,
    start: active.selectionStart ?? null,
    end: active.selectionEnd ?? null,
  };

  redraw();

  if (!mark) return;
  const root = document.getElementById(mark.scope);
  if (!root) return;
  const back = mark.appId ? root.querySelector(`[data-id="${CSS.escape(mark.appId)}"]`)
    : mark.role ? root.querySelector(`[data-role="${CSS.escape(mark.role)}"]`)
    : mark.id ? document.getElementById(mark.id) : null;
  if (!back || back === document.activeElement) return;
  back.focus();
  // A redrawn text field would otherwise put the caret back at the start.
  if (mark.start !== null && back.setSelectionRange) {
    try { back.setSelectionRange(mark.start, mark.end); } catch {}
  }
}

function render() {
  preservingFocus(redraw);
}

function redraw() {
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
  } else if (selectedSystem || creating) {
    // The Platform detail is read-only and the new-application form is
    // being typed into; neither wants redrawing on a poll.
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
  const showPlatform = document.getElementById('show-platform').checked;
  const query = document.getElementById('service-search').value.trim().toLowerCase();
  const services = [...apps, ...(showPlatform ? system : [])];
  const visible = services.filter(a => a.name.toLowerCase().includes(query));
  const empty = services.length === 0 && !query;
  document.getElementById('dashboard-empty').classList.toggle('hidden', !empty);
  document.getElementById('dashboard-content').classList.toggle('hidden', empty);

  const suffix = document.getElementById('dns-suffix').textContent;
  document.getElementById('dashboard-subtitle').textContent = apps.length === 0
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
  rows.innerHTML = visible.map(a => `
    <tr ${a.role ? `data-role="${esc(a.role)}"` : `data-id="${esc(a.id)}"`} tabindex="0" role="button" aria-label="Open ${esc(a.name)}">
      <td>${esc(a.name)}${a.role ? ' <span class="badge badge-neutral">Platform</span>' : ''}</td>
      <td class="mono">${a.role ? '<span class="muted">—</span>' : esc(a.hostname)}${(a.aliases || []).length ? `<span class="t-metadata muted"> +${(a.aliases || []).length}</span>` : ''}</td>
      <td class="mono">${esc(a.image)}</td>
      <td><span class="badge ${statusBadge(a.status)}"><span class="dot" aria-hidden="true"></span>${esc(a.status)}</span></td>
      <td class="num">${a.restarts === null || a.restarts === undefined ? '<span class="muted">—</span>' : a.restarts}</td>
    </tr>
  `).join('') || '<tr><td colspan="5" class="muted">No services match your filters.</td></tr>';

  document.getElementById('app-count').textContent =
    visible.length + ' of ' + services.length + (showPlatform ? ' services' : ' applications');
  rows.querySelectorAll('tr[data-id], tr[data-role]').forEach(tr => {
    const open = () => tr.dataset.role ? selectSystem(tr.dataset.role) : selectApp(tr.dataset.id);
    tr.addEventListener('click', open);
    tr.addEventListener('keydown', e => {
      if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); open(); }
    });
  });
}

function setCrumb(label) {
  document.querySelector('.breadcrumb [aria-current]').textContent = label;
}

function showDashboard() {
  rememberView('overview');
  if (logAbort) { logAbort.abort(); logAbort = null; }
  selected = null;
  selectedSystem = null;
  creating = false;
  detailSignature = null;
  setCrumb('Overview');
  document.querySelector('[data-home]').setAttribute('aria-current', 'page');
  document.getElementById('dashboard').classList.remove('hidden');
  document.getElementById('detail').classList.add('hidden');
  document.querySelectorAll('#sidebar [data-id]').forEach(el => {
    el.removeAttribute('aria-current');
  });
}

function showPlatform() {
  document.getElementById('show-platform').checked = true;
  document.getElementById('service-search').value = '';
  renderDashboard();
  showDashboard();
}

/* The Platform Infra detail is the read-only twin of the Application detail:
   the same split, with the edit form replaced by a .kv definition list. No
   Remove, no Save — the absence of the actions is what says read-only. */
function selectSystem(role) {
  const c = system.find(x => x.role === role);
  if (!c) return;
  rememberView('system', role);
  selected = null;
  selectedSystem = role;
  creating = false;
  detailSignature = null;

  setCrumb('Platform');
  document.querySelector('[data-home]').removeAttribute('aria-current');
  document.getElementById('dashboard').classList.add('hidden');
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
    <div class="logview"><div class="log-scroll" id="logs-content" role="log" aria-live="polite" tabindex="0" aria-label="Logs"></div></div>
  `;

  startSystemLogStream(role);
}

function selectApp(id) {
  const app = apps.find(a => a.id === id);
  if (!app) return;
  rememberView('app', id);
  selected = id;
  selectedSystem = null;
  creating = false;
  detailSignature = appSignature(app);

  document.querySelectorAll('#sidebar [data-id]').forEach(el => {
    if (el.dataset.id === id) el.setAttribute('aria-current', 'page');
    else el.removeAttribute('aria-current');
  });

  setCrumb(app.name);
  document.querySelector('[data-home]').removeAttribute('aria-current');
  document.getElementById('dashboard').classList.add('hidden');
  document.getElementById('detail').classList.remove('hidden');

  // The whole detail is one page: the identity is the page header, and the
  // split below it is only the two panes. Putting the header inside the
  // config pane made the title half a window wide and scroll with the form.
  const canStop = app.status === 'running' || app.status === 'failed';
  const canStart = app.status === 'stopped' || app.status === 'failed';

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
    <div class="card detail-panel">
      <div class="split">
        <div class="pane">
          ${formMarkup(app)}
        </div>
        <div class="splitter" id="splitter" aria-hidden="true"></div>
        <div class="pane pane-logs" id="logs-panel"></div>
      </div>
    </div>
  `;

  document.getElementById('remove-btn').addEventListener('click', () => removeApp(app));
  document.getElementById('app-form').addEventListener('submit', e => saveApp(e, app.id));
  inspected = null;
  wireComposeInspection();
  document.querySelectorAll('[data-lifecycle]').forEach(btn => {
    btn.addEventListener('click', () => lifecycle(app, btn.dataset.lifecycle));
  });
  const retry = document.querySelector('[data-retry]');
  if (retry) retry.addEventListener('click', () => {
    document.getElementById('app-form').requestSubmit();
  });

  // Rendering the detail replaced the log pane, so the stream always restarts:
  // the element the running one holds is no longer in the document. This also
  // keeps a redeploy honest — it replaces the container, and output from the
  // previous one must not keep scrolling past.
  document.getElementById('logs-panel').innerHTML = `
    <div class="field">
      <label for="log-container">Logs from container</label>
      <select class="select mono" id="log-container" disabled><option>Loading…</option></select>
    </div>
    <div class="logview"><div class="log-scroll" id="logs-content" role="log" aria-live="polite" tabindex="0" aria-label="Logs"></div></div>
  `;

  loadAppLogs(app.id);
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
async function loadAppLogs(id) {
  if (logAbort) { logAbort.abort(); logAbort = null; }
  const selector = document.getElementById('log-container');
  const content = document.getElementById('logs-content');
  try {
    const res = await fetch('/apps/id/' + encodeURIComponent(id) + '/containers', { headers: authHeaders() });
    if (!res.ok) throw new Error('Could not load containers.');
    const containers = await res.json();
    if (document.getElementById('log-container') !== selector) return;
    selector.innerHTML = containers.map(name => `<option value="${esc(name)}">${esc(name)}</option>`).join('');
    selector.disabled = containers.length < 2;
    const open = () => {
      openLogStream('/apps/id/' + encodeURIComponent(id) + '/logs?container=' + encodeURIComponent(selector.value));
    };
    selector.addEventListener('change', open);
    if (containers.length) open();
    else content.textContent = 'No containers available.';
    if (containers.length === 1) {
      const heading = document.createElement('p');
      heading.className = 't-caps';
      heading.textContent = 'Logs from ' + containers[0];
      selector.parentElement.replaceWith(heading);
    }
  } catch {
    if (document.getElementById('log-container') !== selector) return;
    selector.innerHTML = '<option>Unavailable</option>';
    content.textContent = 'Could not load containers. Reopen the application to retry.';
  }
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

// ── The form: one for creating, one for editing ────────────────

/* The same form serves "New application" and the detail page. Creating
   offers the choice between an image and a Compose file; editing keeps the
   definition the Application already has. Field ids are the same either
   way, so the readers and the error placement do not care which it is. */
function formMarkup(app) {
  const creating = !app;
  const source = creating ? 'image' : (isCompose(app) ? 'compose' : 'image');
  const suffix = esc(dnsSuffix());

  const imageField = `
    <div class="field" data-source="image">
      <label for="f-image">Container image</label>
      <input class="input mono" type="text" id="f-image" value="${esc(app?.image || '')}" placeholder="nginx:alpine"
             ${source === 'image' ? 'required' : ''} aria-describedby="f-image-help">
      <small class="field-hint" id="f-image-help">One container serving HTTP on port 80.</small>
    </div>`;

  const composeFields = `
    <div class="field ${creating ? 'hidden' : ''}" data-source="compose">
      <label for="f-compose">Compose file</label>
      <textarea class="textarea mono compose-editor" id="f-compose" rows="14" spellcheck="false"
                placeholder="services:&#10;  web:&#10;    image: …"
                ${source === 'compose' ? 'required' : ''}
                aria-describedby="f-compose-help f-compose-error">${esc(app?.compose || '')}</textarea>
      <small class="field-hint" id="f-compose-help">${creating
        ? 'The supported subset is in docs/compose-applications.md. <code>~</code> and <code>./</code> paths land in the application\'s data directory.'
        : 'Saving brings the project up again; only services whose definition changed are recreated.'}</small>
      <small class="field-error hidden" id="f-compose-error"></small>
    </div>
    <div class="field-row ${creating ? 'hidden' : ''}" data-source="compose">
      <div class="field">
        <label for="f-web-service">Web service</label>
        <select class="select mono" id="f-web-service" data-current="${esc(app?.web_service || '')}"></select>
      </div>
      <div class="field">
        <label for="f-web-port">Web port</label>
        <select class="select mono" id="f-web-port" data-current="${app?.web_port || ''}"></select>
        <input class="input mono hidden" type="number" id="f-web-port-custom" min="1" max="65535"
               placeholder="9119" value="${app?.web_port || ''}" aria-label="Web port">
      </div>
    </div>
    <small class="field-hint ${creating ? 'hidden' : ''}" data-source="compose" id="f-web-help">The service and the port it listens on inside its container. The hostname reaches it through Traefik; nothing is published on the Host.</small>
    <div class="field ${creating ? 'hidden' : ''}" data-source="compose" id="f-storage"></div>`;

  const definition = creating
    ? `<fieldset class="field source-choice">
         <legend>Definition</legend>
         <label class="row"><input type="radio" name="f-source" value="image" checked> Container image</label>
         <label class="row"><input type="radio" name="f-source" value="compose"> Compose file</label>
       </fieldset>${imageField}${composeFields}`
    : (source === 'compose' ? composeFields : imageField);

  const actions = creating
    ? `<button id="cancel-btn" type="button" class="btn btn-outline btn-sm">Cancel</button>
       <button type="submit" class="btn btn-primary btn-sm">Deploy</button>`
    : `<button id="remove-btn" type="button" class="btn btn-danger-ghost btn-sm">Remove application</button>
       <button type="submit" class="btn btn-primary btn-sm">Save and redeploy</button>`;

  return `
    <form class="stack" id="app-form">
      <p class="t-caps">Configuration</p>
      <div class="field">
        <label for="f-name">Name</label>
        <input class="input" type="text" id="f-name" value="${esc(app?.name || '')}" placeholder="my-app" required aria-describedby="f-name-help">
        <small class="field-hint" id="f-name-help">Lowercase letters, numbers and hyphens; at most 63 characters.</small>
      </div>
      ${definition}
      <div class="field">
        <label for="f-hostname">Hostname</label>
        <input class="input mono" type="text" id="f-hostname" value="${esc(app?.hostname || '')}" placeholder="my-app.${suffix}"
               ${creating ? '' : 'required'} aria-describedby="f-hostname-help f-hostname-error">
        <small class="field-hint" id="f-hostname-help">${creating
          ? 'Leave empty to use the application name and DNS suffix.'
          : 'Takes effect immediately; the container keeps running.'}</small>
        <small class="field-error hidden" id="f-hostname-error"></small>
      </div>
      <div class="field">
        <label for="f-aliases">Aliases</label>
        <input class="input mono" type="text" id="f-aliases" value="${esc((app?.aliases || []).join(', '))}"
               placeholder="old-name.${suffix}" aria-describedby="f-aliases-help f-aliases-error">
        <small class="field-hint" id="f-aliases-help">Other hostnames this application also answers on, comma separated.${creating ? '' : ' Keep the old one here to change the Hostname without breaking it.'}</small>
        <small class="field-error hidden" id="f-aliases-error"></small>
      </div>
      <div id="form-error" class="hidden"></div>
      ${creating ? '' : servicesMarkup(app)}
      <div class="form-actions">${actions}</div>
    </form>`;
}

/* What the pasted file declares, as the Platform reads it. Asked for as the
   Operator types, so the web service and port come from what is in the file
   rather than from memory — 9191 for 9119 is one keystroke, and a 502. */
let inspected = null;
let inspectTimer = null;

function highlightCompose(textarea) {
  if (!window.Prism?.languages.yaml) return;
  const wrapper = document.createElement('div');
  wrapper.className = 'yaml-editor';
  const highlight = document.createElement('div');
  highlight.className = 'yaml-highlight';
  highlight.setAttribute('aria-hidden', 'true');
  textarea.before(wrapper);
  wrapper.append(highlight, textarea);
  textarea.wrap = 'off';
  const syncScroll = () => {
    highlight.scrollTop = textarea.scrollTop;
    highlight.scrollLeft = textarea.scrollLeft;
  };
  const paint = () => {
    highlight.innerHTML = Prism.highlight(textarea.value + '\n', Prism.languages.yaml, 'yaml');
    syncScroll();
  };
  textarea.addEventListener('input', paint);
  textarea.addEventListener('scroll', syncScroll);
  paint();
}

function wireComposeInspection() {
  const compose = document.getElementById('f-compose');
  const service = document.getElementById('f-web-service');
  const port = document.getElementById('f-web-port');
  if (!compose) return;
  highlightCompose(compose);
  compose.addEventListener('input', () => {
    clearTimeout(inspectTimer);
    inspectTimer = setTimeout(inspectCompose, 400);
  });
  service.addEventListener('change', () => renderWebTarget(true));
  port.addEventListener('change', () => renderPortInput());
  renderInspection();
  if (compose.value.trim()) inspectCompose();
}

async function inspectCompose() {
  const compose = document.getElementById('f-compose');
  if (!compose) return;
  const text = compose.value;
  if (!text.trim()) { inspected = null; renderInspection(); return; }
  try {
    const res = await fetch('/compose/inspect', {
      method: 'POST',
      headers: authHeaders(),
      body: JSON.stringify({ compose: text }),
    });
    // The file may have changed while the request was out; a stale answer
    // would describe a file that is no longer in the editor.
    if (compose.value !== text) return;
    if (!res.ok) {
      inspected = null;
      renderInspection();
      const failure = await failureOf(res);
      showFieldError('f-compose', { error: failure.caused_by[0] || failure.error, caused_by: [] }, false);
      return;
    }
    clearFieldError('f-compose');
    inspected = await res.json();
    renderInspection();
  } catch {
    inspected = null;
  }
}

function inspectedServices() {
  return inspected ? inspected.services : [];
}

function chosenService() {
  const select = document.getElementById('f-web-service');
  return inspectedServices().find(s => s.name === select.value) || null;
}

/* Everything the file implies for the form: the service to route to, the
   port on it, and the storage the Platform will set up. */
function renderInspection() {
  renderWebTarget(false);
  renderStorage();
}

/* The web service is a choice among the file's services, preselected with
   the Platform's own default. With one service there is nothing to choose;
   the field still shows which one, so the port next to it has a subject. */
function renderWebTarget(serviceChanged) {
  const select = document.getElementById('f-web-service');
  if (!select) return;
  const services = inspectedServices();
  const wanted = select.value || select.dataset.current || (inspected && inspected.web_service) || '';

  if (services.length === 0) {
    select.innerHTML = '<option value="">Paste a Compose file first</option>';
    select.disabled = true;
  } else {
    select.innerHTML = services.map(s =>
      `<option value="${esc(s.name)}">${esc(s.name)}${s.ports.length ? '' : ' (no ports)'}</option>`).join('');
    select.disabled = false;
    select.value = services.some(s => s.name === wanted) ? wanted : services[0].name;
  }

  renderPortChoice(serviceChanged);

  const help = document.getElementById('f-web-help');
  const known = chosenService();
  if (!help) return;
  if (known && known.ports.length > 1) {
    help.textContent = known.name + ' listens on ' + known.ports.map(p => p.container).join(', ') +
      ' inside its container. Pick the one the browser should reach; the hostname gets there through Traefik.';
  } else if (known && known.ports.length === 1) {
    help.textContent = known.name + ' listens on ' + known.ports[0].container +
      ' inside its container; the hostname reaches it through Traefik.';
  } else if (known) {
    help.textContent = known.name + ' declares no ports. Type the port it listens on inside its container.';
  } else {
    help.textContent = 'The service and the port it listens on inside its container. The hostname reaches it through Traefik; nothing is published on the Host.';
  }
}

/* The port is a choice among what the service declares, with "Other" for a
   port the file does not mention. A service with no ports gets the number
   input directly: there is nothing to choose from. */
function renderPortChoice(serviceChanged) {
  const select = document.getElementById('f-web-port');
  const custom = document.getElementById('f-web-port-custom');
  if (!select || !custom) return;
  const known = chosenService();
  const ports = known ? [...new Set(known.ports.map(p => String(p.container)))] : [];
  const current = serviceChanged ? '' : (custom.value.trim() || select.dataset.current || '');

  if (ports.length === 0) {
    select.innerHTML = '';
    select.classList.add('hidden');
    custom.classList.remove('hidden');
    if (serviceChanged) custom.value = '';
    return;
  }

  select.innerHTML = ports.map(p => `<option value="${p}">${p}</option>`).join('') +
    '<option value="other">Other…</option>';
  select.classList.remove('hidden');
  if (ports.includes(current)) {
    select.value = current;
  } else if (current) {
    select.value = 'other';
    custom.value = current;
  } else {
    select.value = ports[0];
  }
  renderPortInput();
}

/* "Other" reveals the number input; a listed port hides it and takes its
   value along, so what is read back is always one field. */
function renderPortInput() {
  const select = document.getElementById('f-web-port');
  const custom = document.getElementById('f-web-port-custom');
  if (!select || !custom || select.classList.contains('hidden')) return;
  if (select.value === 'other') {
    custom.classList.remove('hidden');
    custom.focus();
  } else {
    custom.classList.add('hidden');
    custom.value = select.value;
  }
}

/* What each service keeps, and where the Platform puts it. A service with
   no storage at all is worth a sentence: whatever it writes goes with the
   container on the next redeploy. */
function renderStorage() {
  const el = document.getElementById('f-storage');
  if (!el) return;
  const services = inspectedServices();
  if (services.length === 0) { el.innerHTML = ''; return; }

  const rows = [];
  for (const s of services) {
    if (s.volumes.length === 0) {
      rows.push(`<li><span class="mono">${esc(s.name)}</span> keeps nothing: what it writes is lost when its container is recreated.</li>`);
      continue;
    }
    for (const v of s.volumes) {
      let where;
      if (v.kind === 'data') where = `the application's directory, <span class="mono">${esc(v.data_path)}</span>`;
      else if (v.kind === 'named') where = `a named volume, <span class="mono">${esc(v.source)}</span>`;
      else if (v.kind === 'host') where = `the Host path <span class="mono">${esc(v.source)}</span>, as written`;
      else where = 'an anonymous volume, gone with the container';
      rows.push(`<li><span class="mono">${esc(s.name)}</span>: <span class="mono">${esc(v.target)}</span> lives in ${where}.</li>`);
    }
  }
  el.innerHTML = `
    <p class="t-caps">Storage</p>
    <ul class="storage-list">${rows.join('')}</ul>
    <small class="field-hint">Created on deploy and kept across redeploys, restarts and removal.</small>`;
}

/* The two definitions share the form. Only the visible one is required, or
   the browser would refuse to submit over a field the operator cannot see. */
function showSource(source) {
  document.querySelectorAll('#app-form [data-source]').forEach(el => {
    el.classList.toggle('hidden', el.dataset.source !== source);
  });
  document.getElementById('f-image').required = source === 'image';
  document.getElementById('f-compose').required = source === 'compose';
}

function formSource() {
  const radio = document.querySelector('input[name="f-source"]:checked');
  if (radio) return radio.value;
  return document.getElementById('f-compose') ? 'compose' : 'image';
}

/* What the form says, as the API wants it. */
function readForm() {
  const source = formSource();
  const body = {
    name: document.getElementById('f-name').value.trim(),
    aliases: parseAliases(document.getElementById('f-aliases').value),
  };
  const hostname = document.getElementById('f-hostname').value.trim();
  if (hostname) body.hostname = hostname;
  if (source === 'compose') {
    body.compose = document.getElementById('f-compose').value;
    body.web_service = document.getElementById('f-web-service').value.trim();
    // The number input always carries the answer: a listed port copies
    // itself into it, and "Other" is typed straight into it.
    const port = document.getElementById('f-web-port-custom').value.trim();
    if (port) body.web_port = Number(port);
  } else {
    body.image = document.getElementById('f-image').value.trim();
  }
  return { source, body };
}

function clearFormErrors() {
  document.getElementById('form-error').className = 'hidden';
  for (const id of ['f-hostname', 'f-aliases', 'f-compose']) {
    if (document.getElementById(id)) clearFieldError(id);
  }
}

/* Puts a refusal on the field it is about, or under the form when it is
   about nothing in particular. Returns false when nothing was shown. */
function showFormFailure(failure, aliases) {
  if (failure.error.startsWith('invalid Application Hostname:')) {
    showFieldError(routingErrorField(failure, aliases), failure);
    return true;
  }
  if (failure.error.startsWith('invalid Compose definition') && document.getElementById('f-compose')) {
    showFieldError('f-compose', { error: failure.caused_by[0] || failure.error, caused_by: [] });
    return true;
  }
  showAlert(document.getElementById('form-error'), failure);
  return true;
}

// ── New application ─────────────────────────────────────────────
function showNewApp() {
  rememberView('new');
  if (logAbort) { logAbort.abort(); logAbort = null; }
  selected = null;
  selectedSystem = null;
  creating = true;
  detailSignature = null;

  setCrumb('New application');
  document.querySelector('[data-home]').removeAttribute('aria-current');
  document.querySelectorAll('#sidebar [data-id]').forEach(el => el.removeAttribute('aria-current'));
  document.getElementById('dashboard').classList.add('hidden');
  document.getElementById('detail').classList.remove('hidden');

  document.getElementById('detail').innerHTML = `
    <div class="page-header">
      <h1 class="t-h1">New application</h1>
      <p class="muted t-label">From a container image or a Compose file. It will be reachable on ${esc(dnsSuffix())}.</p>
    </div>
    <div class="card form-page">
      <div class="card-body">
        ${formMarkup(null)}
      </div>
    </div>
  `;

  document.querySelectorAll('input[name="f-source"]').forEach(radio => {
    radio.addEventListener('change', () => showSource(radio.value));
  });
  document.getElementById('cancel-btn').addEventListener('click', showDashboard);
  document.getElementById('app-form').addEventListener('submit', deploy);
  inspected = null;
  wireComposeInspection();
  document.getElementById('f-name').focus();
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

function showFieldError(inputId, failure, focus = true) {
  const input = document.getElementById(inputId);
  const error = document.getElementById(inputId + '-error');
  input.setAttribute('aria-invalid', 'true');
  error.textContent = asReport(failure).error;
  error.classList.remove('hidden');
  if (focus) input.focus();
}

function routingErrorField(failure, aliases) {
  const message = asReport(failure).error;
  const conflictingAlias = aliases.some(alias => message.includes("'" + alias + "'"));
  return conflictingAlias ? 'f-aliases' : 'f-hostname';
}

async function saveApp(e, id) {
  e.preventDefault();
  clearFormErrors();
  const { body } = readForm();
  // Editing always sends the Hostname: an empty one is a mistake here, not
  // a request for the default.
  body.hostname = document.getElementById('f-hostname').value.trim();

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

  // The record already carries the reason when the deploy itself failed, and
  // selectApp rendered it. Only errors that never reached the database — a
  // rejected name, a clash, a refused file — need saying here.
  const saved = apps.find(a => a.id === id);
  if (saved && saved.last_error && !failure.error.startsWith('invalid')) return;
  showFormFailure(failure, body.aliases);
}

async function deploy(e) {
  e.preventDefault();
  clearFormErrors();
  const { source, body } = readForm();
  const name = body.name;

  try {
    const res = await fetch('/apps', {
      method: 'POST',
      headers: authHeaders(),
      body: JSON.stringify(body),
    });
    const failure = res.ok ? null : await failureOf(res);

    await loadApps();
    const created = apps.find(a => a.name === name);

    // A refused name or file never reached the database. Keep the form with
    // what was typed, so the fix is one edit rather than a retype.
    if (failure && !created) {
      showFormFailure(failure, body.aliases);
      return;
    }

    // The Application is on record even when the deploy failed, so open it
    // instead of reporting an error the operator cannot act on.
    if (created) selectApp(created.id);
    else showDashboard();

    if (!failure) {
      // Accepted, not finished: the pull runs on the platform and the poll
      // says how it went.
      toast('success', 'Deploy started for ' + name,
            source === 'compose' ? 'Bringing the Compose project up ...' : 'Pulling image ' + body.image + ' ...');
    }
    // A failure needs no toast: selectApp already rendered the reason as the
    // application's alert, and that one stays on screen.
  } catch (err) {
    showAlert(document.getElementById('form-error'), { error: 'Could not reach the platform', caused_by: [err.message] });
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

    // Where the keyboard was standing when the dialog opened, so answering
    // it puts the Operator back there instead of at the top of the page.
    const opener = document.activeElement;

    const close = (answer) => {
      modal.classList.add('hidden');
      accept.removeEventListener('click', onAccept);
      cancel.removeEventListener('click', onCancel);
      modal.removeEventListener('click', onBackdrop);
      document.removeEventListener('keydown', onKey);
      if (opener && opener.isConnected) opener.focus();
      resolve(answer);
    };
    const onAccept = () => close(true);
    const onCancel = () => close(false);
    const onBackdrop = (e) => { if (e.target === modal) close(false); };

    /* Tab stays between the two answers. The page behind the overlay is
       still focusable, and a keyboard user who tabs into it is typing at
       something they cannot see. */
    const onKey = (e) => {
      if (e.key === 'Escape') { close(false); return; }
      if (e.key !== 'Tab') return;
      e.preventDefault();
      (document.activeElement === accept ? cancel : accept).focus();
    };

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
