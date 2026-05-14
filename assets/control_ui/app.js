/* Manta Control UI — vanilla JS dashboard */

const API = {
  async fetch(path, opts = {}) {
    const res = await fetch(path, { headers: { 'Accept': 'application/json' }, ...opts });
    if (!res.ok) throw new Error(`${res.status}: ${await res.text()}`);
    return res.json();
  },
  get: (path) => API.fetch(path),
  post: (path, body) => API.fetch(path, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) }),
  put: (path, body) => API.fetch(path, { method: 'PUT', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) }),
  delete: (path) => API.fetch(path, { method: 'DELETE' }),
};

/* ── Page registry ── */
const pages = {
  dashboard: { title: 'Dashboard', render: renderDashboard },
  agents:    { title: 'Agents',    render: renderAgents },
  channels:  { title: 'Channels',  render: renderChannels },
  providers: { title: 'Providers', render: renderProviders },
  memory:    { title: 'Memory',    render: renderMemory },
  cron:      { title: 'Cron Jobs', render: renderCron },
  config:    { title: 'Config',    render: renderConfig },
};

let currentPage = 'dashboard';

/* ── Router ── */
function navigate(page) {
  if (!pages[page]) page = 'dashboard';
  currentPage = page;
  document.getElementById('pageTitle').textContent = pages[page].title;
  document.querySelectorAll('.nav-item').forEach(el => el.classList.toggle('active', el.dataset.page === page));
  const content = document.getElementById('content');
  content.innerHTML = '<div class="loading"><span class="spinner"></span>Loading...</div>';
  pages[page].render(content);
  window.location.hash = page;
}

document.addEventListener('DOMContentLoaded', () => {
  document.querySelectorAll('.nav-item').forEach(el => {
    el.addEventListener('click', (e) => { e.preventDefault(); navigate(el.dataset.page); });
  });
  document.getElementById('refreshBtn').addEventListener('click', () => navigate(currentPage));
  const initial = window.location.hash.replace('#', '') || 'dashboard';
  navigate(initial);
  checkHealth();
  setInterval(checkHealth, 10000);
});

async function checkHealth() {
  try {
    const data = await API.get('/health');
    const dot = document.getElementById('healthDot');
    const txt = document.getElementById('healthText');
    const ok = data.status === 'healthy';
    dot.className = 'status-dot ' + (ok ? 'online' : 'offline');
    txt.textContent = ok ? 'Online' : 'Degraded';
  } catch {
    document.getElementById('healthDot').className = 'status-dot offline';
    document.getElementById('healthText').textContent = 'Offline';
  }
}

/* ── Dashboard ── */
async function renderDashboard(el) {
  try {
    const [status, health, agents, channels, providers, cronJobs] = await Promise.all([
      API.get('/api/v1/status').catch(() => ({})),
      API.get('/health').catch(() => ({})),
      API.get('/api/v1/agents').catch(() => []),
      API.get('/api/v1/channels').catch(() => []),
      API.get('/api/v1/providers').catch(() => []),
      API.get('/api/v1/cron').catch(() => []),
    ]);

    const metrics = [
      { title: 'Agents', value: Array.isArray(agents) ? agents.length : '-', sub: 'active' },
      { title: 'Channels', value: Array.isArray(channels) ? channels.length : '-', sub: 'configured' },
      { title: 'Providers', value: Array.isArray(providers) ? providers.length : '-', sub: 'available' },
      { title: 'Cron Jobs', value: Array.isArray(cronJobs) ? cronJobs.length : '-', sub: 'scheduled' },
    ];

    el.innerHTML = `
      <div class="card-grid">${metrics.map(m => `
        <div class="card">
          <div class="card-header"><span class="card-title">${m.title}</span></div>
          <div class="card-value">${m.value}</div>
          <div class="card-sub">${m.sub}</div>
        </div>
      `).join('')}</div>

      <div class="table-container">
        <div class="table-header"><h3>Health Status</h3></div>
        <table>
          <thead><tr><th>Subsystem</th><th>Status</th></tr></thead>
          <tbody>
            ${health.subsystems ? Object.entries(health.subsystems).map(([k, v]) => `
              <tr><td>${k}</td><td>${badge(v.status || v)}</td></tr>
            `).join('') : '<tr><td colspan="2">No data</td></tr>'}
          </tbody>
        </table>
      </div>
    `;
  } catch (e) {
    el.innerHTML = errorAlert(e.message);
  }
}

/* ── Agents ── */
async function renderAgents(el) {
  try {
    const agents = await API.get('/api/v1/agents');
    const list = Array.isArray(agents) ? agents : agents.agents || [];
    el.innerHTML = `
      <div class="table-container">
        <div class="table-header"><h3>Agents</h3></div>
        <table>
          <thead><tr><th>ID</th><th>Name</th><th>Status</th><th>Model</th><th>Actions</th></tr></thead>
          <tbody>
            ${list.length ? list.map(a => `
              <tr>
                <td>${a.id || a.agent_id || '-'}</td>
                <td>${a.name || a.personality || '-'}</td>
                <td>${badge('active', 'success')}</td>
                <td>${a.model || '-'}</td>
                <td><button class="btn btn-sm" onclick="alert('Agent: ${a.id}')">Details</button></td>
              </tr>
            `).join('') : '<tr><td colspan="5" class="empty-state">No agents found</td></tr>'}
          </tbody>
        </table>
      </div>
    `;
  } catch (e) {
    el.innerHTML = errorAlert(e.message);
  }
}

/* ── Channels ── */
async function renderChannels(el) {
  try {
    const channels = await API.get('/api/v1/channels');
    const list = Array.isArray(channels) ? channels : [];
    el.innerHTML = `
      <div class="table-container">
        <div class="table-header"><h3>Channels</h3></div>
        <table>
          <thead><tr><th>Name</th><th>Type</th><th>Enabled</th></tr></thead>
          <tbody>
            ${list.length ? list.map(c => `
              <tr>
                <td>${c.name || c.channel_name || '-'}</td>
                <td>${c.channel_type || c.type || '-'}</td>
                <td>${badge(c.enabled ? 'yes' : 'no', c.enabled ? 'success' : 'muted')}</td>
              </tr>
            `).join('') : '<tr><td colspan="3" class="empty-state">No channels configured</td></tr>'}
          </tbody>
        </table>
      </div>
    `;
  } catch (e) {
    el.innerHTML = errorAlert(e.message);
  }
}

/* ── Providers ── */
async function renderProviders(el) {
  try {
    const providers = await API.get('/api/v1/providers');
    const list = Array.isArray(providers) ? providers : [];
    el.innerHTML = `
      <div class="table-container">
        <div class="table-header"><h3>LLM Providers</h3></div>
        <table>
          <thead><tr><th>Name</th><th>Type</th><th>Health</th><th>Actions</th></tr></thead>
          <tbody>
            ${list.length ? list.map(p => `
              <tr>
                <td>${p.name || p.id || '-'}</td>
                <td>${p.provider_type || p.type || '-'}</td>
                <td>${badge('unknown', 'muted')}</td>
                <td>
                  <button class="btn btn-sm" onclick="checkProvider('${p.name || p.id}')">Check</button>
                </td>
              </tr>
            `).join('') : '<tr><td colspan="4" class="empty-state">No providers configured</td></tr>'}
          </tbody>
        </table>
      </div>
    `;
  } catch (e) {
    el.innerHTML = errorAlert(e.message);
  }
}

window.checkProvider = async function(id) {
  try {
    const res = await API.post(`/api/v1/providers/${id}/check`);
    alert(JSON.stringify(res, null, 2));
  } catch (e) {
    alert('Check failed: ' + e.message);
  }
};

/* ── Memory ── */
async function renderMemory(el) {
  try {
    const collections = await API.get('/api/v1/memory/collections');
    const list = Array.isArray(collections) ? collections : [];
    el.innerHTML = `
      <div class="card-grid">
        <div class="card">
          <div class="card-header"><span class="card-title">Collections</span></div>
          <div class="card-value">${list.length}</div>
        </div>
      </div>
      <div class="table-container">
        <div class="table-header"><h3>Memory Collections</h3></div>
        <table>
          <thead><tr><th>Name</th></tr></thead>
          <tbody>
            ${list.length ? list.map(c => `<tr><td>${c}</td></tr>`).join('') : '<tr><td class="empty-state">No collections</td></tr>'}
          </tbody>
        </table>
      </div>
    `;
  } catch (e) {
    el.innerHTML = errorAlert(e.message);
  }
}

/* ── Cron ── */
async function renderCron(el) {
  try {
    const jobs = await API.get('/api/v1/cron');
    const list = Array.isArray(jobs) ? jobs : [];
    el.innerHTML = `
      <div class="table-container">
        <div class="table-header"><h3>Cron Jobs</h3></div>
        <table>
          <thead><tr><th>ID</th><th>Schedule</th><th>Command</th><th>Enabled</th><th>Actions</th></tr></thead>
          <tbody>
            ${list.length ? list.map(j => `
              <tr>
                <td>${j.id || j.job_id || '-'}</td>
                <td><code>${j.schedule || '-'}</code></td>
                <td>${j.command || j.skill_id || '-'}</td>
                <td>${badge(j.enabled ? 'yes' : 'no', j.enabled ? 'success' : 'muted')}</td>
                <td>
                  <button class="btn btn-sm btn-primary" onclick="runCron('${j.id}')">Run</button>
                  <button class="btn btn-sm btn-danger" onclick="deleteCron('${j.id}')">Delete</button>
                </td>
              </tr>
            `).join('') : '<tr><td colspan="5" class="empty-state">No cron jobs</td></tr>'}
          </tbody>
        </table>
      </div>
    `;
  } catch (e) {
    el.innerHTML = errorAlert(e.message);
  }
}

window.runCron = async function(id) {
  try { await API.post(`/api/v1/cron/${id}/run`); alert('Job triggered'); navigate('cron'); }
  catch (e) { alert('Failed: ' + e.message); }
};
window.deleteCron = async function(id) {
  if (!confirm('Delete this cron job?')) return;
  try { await API.delete(`/api/v1/cron/${id}`); navigate('cron'); }
  catch (e) { alert('Failed: ' + e.message); }
};

/* ── Config ── */
async function renderConfig(el) {
  try {
    const config = await API.get('/api/v1/config');
    el.innerHTML = `
      <div class="card">
        <div class="card-header"><span class="card-title">Gateway Configuration</span></div>
        <div class="form-group">
          <label>Edit Config (JSON)</label>
          <textarea id="configEditor">${JSON.stringify(config, null, 2)}</textarea>
        </div>
        <div style="display:flex;gap:8px;margin-top:12px;">
          <button class="btn btn-primary" onclick="saveConfig()">Save &amp; Persist</button>
          <button class="btn" onclick="validateConfig()">Validate Only</button>
        </div>
        <div id="configFeedback"></div>
      </div>
    `;
  } catch (e) {
    el.innerHTML = errorAlert(e.message);
  }
}

window.saveConfig = async function() {
  const raw = document.getElementById('configEditor').value;
  const fb = document.getElementById('configFeedback');
  try {
    const config = JSON.parse(raw);
    const res = await API.put('/api/v1/config', config);
    fb.innerHTML = successAlert('Config saved: ' + JSON.stringify(res));
  } catch (e) {
    fb.innerHTML = errorAlert(e.message);
  }
};

window.validateConfig = async function() {
  const raw = document.getElementById('configEditor').value;
  const fb = document.getElementById('configFeedback');
  try {
    const config = JSON.parse(raw);
    const res = await API.post('/api/v1/config/validate', config);
    fb.innerHTML = successAlert(res.message || 'Config is valid');
  } catch (e) {
    fb.innerHTML = errorAlert(e.message);
  }
};

/* ── Helpers ── */
function badge(text, type = 'muted') {
  const map = { success: 'badge-success', warning: 'badge-warning', danger: 'badge-danger', muted: 'badge-muted' };
  return `<span class="badge ${map[type] || map.muted}">${text}</span>`;
}
function errorAlert(msg) {
  return `<div class="alert alert-error">Error: ${escapeHtml(msg)}</div>`;
}
function successAlert(msg) {
  return `<div class="alert alert-success">${escapeHtml(msg)}</div>`;
}
function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}
