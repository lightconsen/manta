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
  pairing:   { title: 'Pairing',   render: renderPairing },
  gate:      { title: 'Command Gate', render: renderGate },
  audit:     { title: 'Audit Log', render: renderAudit },
  subagents: { title: 'Subagents', render: renderSubagents },
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

/* ── Pairing ── */
async function renderPairing(el) {
  try {
    const [pending, authorized] = await Promise.all([
      API.get('/api/v1/pairing/pending').catch(() => []),
      API.get('/api/v1/pairing/authorized').catch(() => []),
    ]);

    const pendingList = Array.isArray(pending) ? pending : [];
    const authList = Array.isArray(authorized) ? authorized : [];

    el.innerHTML = `
      <div class="card-grid">
        <div class="card">
          <div class="card-header"><span class="card-title">Pending</span></div>
          <div class="card-value">${pendingList.length}</div>
        </div>
        <div class="card">
          <div class="card-header"><span class="card-title">Authorized</span></div>
          <div class="card-value">${authList.length}</div>
        </div>
      </div>

      <div style="display:flex;gap:12px;margin-bottom:16px;">
        <button class="btn btn-primary" onclick="showAddAllowlistModal()">+ Add to Allowlist</button>
      </div>

      <div class="table-container" style="margin-bottom:24px;">
        <div class="table-header"><h3>Pending Requests</h3></div>
        <table>
          <thead><tr><th>Channel</th><th>User ID</th><th>Username</th><th>Code</th><th>Created</th><th>Actions</th></tr></thead>
          <tbody>
            ${pendingList.length ? pendingList.map(r => `
              <tr>
                <td>${r.channel || '-'}</td>
                <td>${r.user_id || '-'}</td>
                <td>${r.username || '-'}</td>
                <td><code>${r.code || '-'}</code></td>
                <td>${r.created_at ? new Date(r.created_at.secs_since_epoch * 1000).toLocaleString() : '-'}</td>
                <td>
                  <button class="btn btn-sm btn-primary" onclick="approvePairing('${r.channel}', '${r.code}')">Approve</button>
                  <button class="btn btn-sm btn-danger" onclick="rejectPairing('${r.channel}', '${r.code}')">Reject</button>
                </td>
              </tr>
            `).join('') : '<tr><td colspan="6" class="empty-state">No pending requests</td></tr>'}
          </tbody>
        </table>
      </div>

      <div class="table-container">
        <div class="table-header"><h3>Authorized Users</h3></div>
        <table>
          <thead><tr><th>Channel</th><th>User ID</th><th>Username</th><th>Approved By</th><th>Authorized At</th><th>Actions</th></tr></thead>
          <tbody>
            ${authList.length ? authList.map(u => `
              <tr>
                <td>${u.channel || '-'}</td>
                <td>${u.user_id || '-'}</td>
                <td>${u.username || '-'}</td>
                <td>${u.approved_by || '-'}</td>
                <td>${u.authorized_at ? new Date(u.authorized_at.secs_since_epoch * 1000).toLocaleString() : '-'}</td>
                <td>
                  <button class="btn btn-sm btn-danger" onclick="revokePairing('${u.channel}', '${u.user_id}')">Revoke</button>
                </td>
              </tr>
            `).join('') : '<tr><td colspan="6" class="empty-state">No authorized users</td></tr>'}
          </tbody>
        </table>
      </div>

      <div id="allowlistModal" style="display:none;position:fixed;top:0;left:0;right:0;bottom:0;background:rgba(0,0,0,0.5);align-items:center;justify-content:center;z-index:100;">
        <div class="card" style="width:400px;max-width:90%;">
          <div class="card-header"><span class="card-title">Add to Allowlist</span></div>
          <div class="form-group">
            <label>Channel</label>
            <input type="text" id="allowlistChannel" placeholder="telegram">
          </div>
          <div class="form-group">
            <label>User ID</label>
            <input type="text" id="allowlistUserId" placeholder="123456">
          </div>
          <div class="form-group">
            <label>Username (optional)</label>
            <input type="text" id="allowlistUsername" placeholder="@alice">
          </div>
          <div style="display:flex;gap:8px;justify-content:flex-end;">
            <button class="btn" onclick="hideAddAllowlistModal()">Cancel</button>
            <button class="btn btn-primary" onclick="submitAllowlist()">Add</button>
          </div>
        </div>
      </div>
    `;
  } catch (e) {
    el.innerHTML = errorAlert(e.message);
  }
}

window.approvePairing = async function(channel, code) {
  try {
    await API.post('/api/v1/pairing/approve', { channel, code });
    alert('User approved');
    navigate('pairing');
  } catch (e) { alert('Failed: ' + e.message); }
};

window.rejectPairing = async function(channel, code) {
  if (!confirm('Reject this pairing request?')) return;
  try {
    await API.post('/api/v1/pairing/reject', { channel, code });
    navigate('pairing');
  } catch (e) { alert('Failed: ' + e.message); }
};

window.revokePairing = async function(channel, userId) {
  if (!confirm('Revoke access for this user?')) return;
  try {
    await API.post('/api/v1/pairing/revoke', { channel, user_id: userId });
    navigate('pairing');
  } catch (e) { alert('Failed: ' + e.message); }
};

window.showAddAllowlistModal = function() {
  document.getElementById('allowlistModal').style.display = 'flex';
};
window.hideAddAllowlistModal = function() {
  document.getElementById('allowlistModal').style.display = 'none';
};
window.submitAllowlist = async function() {
  const channel = document.getElementById('allowlistChannel').value;
  const userId = document.getElementById('allowlistUserId').value;
  const username = document.getElementById('allowlistUsername').value || undefined;
  if (!channel || !userId) { alert('Channel and User ID are required'); return; }
  try {
    await API.post('/api/v1/pairing/allowlist', { channel, user_id: userId, username });
    hideAddAllowlistModal();
    navigate('pairing');
  } catch (e) { alert('Failed: ' + e.message); }
};

/* ── Command Gate ── */
async function renderGate(el) {
  try {
    const data = await API.get('/api/v1/gate/levels').catch(() => ({}));
    const levels = data.levels || {};
    const entries = Object.entries(levels);

    el.innerHTML = `
      <div style="display:flex;gap:12px;margin-bottom:16px;">
        <button class="btn btn-primary" onclick="showSetGateModal()">+ Set Level</button>
      </div>
      <div class="table-container">
        <div class="table-header"><h3>User Levels</h3></div>
        <table>
          <thead><tr><th>User ID</th><th>Level</th><th>Actions</th></tr></thead>
          <tbody>
            ${entries.length ? entries.map(([userId, level]) => `
              <tr>
                <td>${userId}</td>
                <td>${badge(level, level === 'admin' ? 'danger' : level === 'user' ? 'success' : 'muted')}</td>
                <td><button class="btn btn-sm btn-danger" onclick="clearGateLevel('${userId}')">Clear</button></td>
              </tr>
            `).join('') : '<tr><td colspan="3" class="empty-state">No custom levels configured</td></tr>'}
          </tbody>
        </table>
      </div>
      <div class="card" style="margin-top:24px;">
        <div class="card-header"><span class="card-title">About Command Gate</span></div>
        <div style="font-size:13px;color:var(--text-secondary);line-height:1.6;">
          <p><strong>Chat</strong> — Can send messages but cannot invoke slash commands.</p>
          <p><strong>User</strong> — Can send messages and invoke user-level commands (e.g., <code>/skill list</code>).</p>
          <p><strong>Admin</strong> — Full access including admin-only commands (e.g., <code>/admin providers</code>).</p>
          <p>Unknown users default to <strong>Chat</strong> level.</p>
        </div>
      </div>
      <div id="gateModal" style="display:none;position:fixed;top:0;left:0;right:0;bottom:0;background:rgba(0,0,0,0.5);align-items:center;justify-content:center;z-index:100;">
        <div class="card" style="width:400px;max-width:90%;">
          <div class="card-header"><span class="card-title">Set User Level</span></div>
          <div class="form-group">
            <label>User ID</label>
            <input type="text" id="gateUserId" placeholder="user123">
          </div>
          <div class="form-group">
            <label>Level</label>
            <select id="gateLevel">
              <option value="chat">Chat</option>
              <option value="user">User</option>
              <option value="admin">Admin</option>
            </select>
          </div>
          <div style="display:flex;gap:8px;justify-content:flex-end;">
            <button class="btn" onclick="hideSetGateModal()">Cancel</button>
            <button class="btn btn-primary" onclick="submitGateLevel()">Save</button>
          </div>
        </div>
      </div>
    `;
  } catch (e) {
    el.innerHTML = errorAlert(e.message);
  }
}

window.showSetGateModal = function() {
  document.getElementById('gateModal').style.display = 'flex';
};
window.hideSetGateModal = function() {
  document.getElementById('gateModal').style.display = 'none';
};
window.submitGateLevel = async function() {
  const userId = document.getElementById('gateUserId').value;
  const level = document.getElementById('gateLevel').value;
  if (!userId) { alert('User ID is required'); return; }
  try {
    await API.post('/api/v1/gate/levels', { user_id: userId, level });
    hideSetGateModal();
    navigate('gate');
  } catch (e) { alert('Failed: ' + e.message); }
};
window.clearGateLevel = async function(userId) {
  if (!confirm('Clear custom level for ' + userId + '?')) return;
  try {
    await API.delete('/api/v1/gate/levels/' + userId);
    navigate('gate');
  } catch (e) { alert('Failed: ' + e.message); }
};

/* ── Audit Log ── */
async function renderAudit(el) {
  try {
    const data = await API.get('/api/v1/audit/log');
    const entries = data.entries || [];

    el.innerHTML = `
      <div class="card-grid">
        <div class="card">
          <div class="card-header"><span class="card-title">Total Entries</span></div>
          <div class="card-value">${data.count || 0}</div>
        </div>
      </div>

      <div class="table-container">
        <div class="table-header"><h3>Recent Audit Events</h3></div>
        <table>
          <thead><tr><th>Time</th><th>Type</th><th>Actor</th><th>Target</th><th>Allowed</th><th>Description</th></tr></thead>
          <tbody>
            ${entries.length ? entries.map(e => `
              <tr>
                <td>${e.timestamp ? new Date(e.timestamp.secs_since_epoch * 1000).toLocaleString() : '-'}</td>
                <td>${badge(e.event_type || '-')}</td>
                <td>${e.actor || '-'}</td>
                <td>${e.target || '-'}</td>
                <td>${badge(e.allowed ? 'yes' : 'no', e.allowed ? 'success' : 'danger')}</td>
                <td>${e.description || '-'}</td>
              </tr>
            `).join('') : '<tr><td colspan="6" class="empty-state">No audit entries</td></tr>'}
          </tbody>
        </table>
      </div>
    `;
  } catch (e) {
    el.innerHTML = errorAlert(e.message);
  }
}

/* ── Subagents ── */
async function renderSubagents(el) {
  try {
    const sessions = await API.get('/api/v1/acp/sessions').catch(() => []);
    const list = Array.isArray(sessions) ? sessions : sessions.sessions || [];

    el.innerHTML = `
      <div style="display:flex;gap:12px;margin-bottom:16px;">
        <button class="btn btn-primary" onclick="showSpawnSubagentModal()">+ Spawn Subagent</button>
      </div>
      <div class="table-container">
        <div class="table-header"><h3>Active Subagents</h3></div>
        <table>
          <thead><tr><th>ID</th><th>Session</th><th>Parent</th><th>Mode</th><th>Status</th><th>Actions</th></tr></thead>
          <tbody>
            ${list.length ? list.map(s => `
              <tr>
                <td>${s.subagent_id || s.id || '-'}</td>
                <td><code>${s.session_id || '-'}</code></td>
                <td>${s.parent_id || '-'}</td>
                <td>${s.mode || '-'}</td>
                <td>${badge(s.status || 'unknown', s.status === 'Running' ? 'success' : 'muted')}</td>
                <td>
                  <button class="btn btn-sm btn-primary" onclick="showSendMessageModal('${s.session_id || s.id}')">Message</button>
                  <button class="btn btn-sm btn-danger" onclick="terminateSubagent('${s.session_id || s.id}')">Terminate</button>
                </td>
              </tr>
            `).join('') : '<tr><td colspan="6" class="empty-state">No active subagents</td></tr>'}
          </tbody>
        </table>
      </div>

      <div id="spawnModal" style="display:none;position:fixed;top:0;left:0;right:0;bottom:0;background:rgba(0,0,0,0.5);align-items:center;justify-content:center;z-index:100;">
        <div class="card" style="width:400px;max-width:90%;">
          <div class="card-header"><span class="card-title">Spawn Subagent</span></div>
          <div class="form-group">
            <label>Task</label>
            <input type="text" id="spawnTask" placeholder="Research quantum computing">
          </div>
          <div class="form-group">
            <label>Mode</label>
            <select id="spawnMode">
              <option value="run">Run</option>
              <option value="session">Session</option>
            </select>
          </div>
          <div style="display:flex;gap:8px;justify-content:flex-end;">
            <button class="btn" onclick="hideSpawnModal()">Cancel</button>
            <button class="btn btn-primary" onclick="submitSpawn()">Spawn</button>
          </div>
        </div>
      </div>

      <div id="messageModal" style="display:none;position:fixed;top:0;left:0;right:0;bottom:0;background:rgba(0,0,0,0.5);align-items:center;justify-content:center;z-index:100;">
        <div class="card" style="width:400px;max-width:90%;">
          <div class="card-header"><span class="card-title">Send Message</span></div>
          <input type="hidden" id="messageSessionId">
          <div class="form-group">
            <label>Message</label>
            <input type="text" id="messageText" placeholder="Hello subagent">
          </div>
          <div style="display:flex;gap:8px;justify-content:flex-end;">
            <button class="btn" onclick="hideMessageModal()">Cancel</button>
            <button class="btn btn-primary" onclick="submitMessage()">Send</button>
          </div>
        </div>
      </div>
    `;
  } catch (e) {
    el.innerHTML = errorAlert(e.message);
  }
}

window.showSpawnSubagentModal = function() {
  document.getElementById('spawnModal').style.display = 'flex';
};
window.hideSpawnModal = function() {
  document.getElementById('spawnModal').style.display = 'none';
};
window.submitSpawn = async function() {
  const task = document.getElementById('spawnTask').value;
  const mode = document.getElementById('spawnMode').value;
  if (!task) { alert('Task is required'); return; }
  try {
    await API.post('/api/v1/acp/sessions', { task, mode });
    hideSpawnModal();
    navigate('subagents');
  } catch (e) { alert('Failed: ' + e.message); }
};

window.showSendMessageModal = function(sessionId) {
  document.getElementById('messageSessionId').value = sessionId;
  document.getElementById('messageModal').style.display = 'flex';
};
window.hideMessageModal = function() {
  document.getElementById('messageModal').style.display = 'none';
};
window.submitMessage = async function() {
  const sessionId = document.getElementById('messageSessionId').value;
  const message = document.getElementById('messageText').value;
  if (!message) { alert('Message is required'); return; }
  try {
    await API.post(`/api/v1/acp/sessions/${sessionId}/message`, { message });
    hideMessageModal();
    alert('Message sent');
  } catch (e) { alert('Failed: ' + e.message); }
};
window.terminateSubagent = async function(sessionId) {
  if (!confirm('Terminate this subagent?')) return;
  try {
    await API.delete(`/api/v1/acp/sessions/${sessionId}`);
    navigate('subagents');
  } catch (e) { alert('Failed: ' + e.message); }
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
