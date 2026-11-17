const $ = sel => document.querySelector(sel);
const $$ = sel => document.querySelectorAll(sel);

const state = {
  view: 'now',
  search: '',
  data: null,
  selected: null,
};

async function load() {
  const params = new URLSearchParams({ view: state.view, limit: '50' });
  if (state.search) params.set('search', state.search);
  const token = getHashToken();
  if (!token) { showError('缺少认证令牌，请从 workmesh ui 启动的地址访问。'); return; }
  try {
    const res = await fetch(`/api/state?${params}`, { headers: { Authorization: `Bearer ${token}` } });
    if (!res.ok) throw new Error(await res.text());
    state.data = await res.json();
    render();
    $('#last-refresh').textContent = new Date().toLocaleTimeString();
  } catch (err) { showError(err.message); }
}

function getHashToken() {
  const hash = location.hash.slice(1);
  if (hash.startsWith('token=')) {
    const token = hash.slice(6);
    history.replaceState(null, '', location.pathname);
    sessionStorage.setItem('wm-token', token);
    return token;
  }
  return sessionStorage.getItem('wm-token');
}

function showError(msg) {
  $('#items').innerHTML = `<div class="empty"><div class="icon">⚠</div><p>${esc(msg)}</p></div>`;
}

function esc(s) {
  const d = document.createElement('div');
  d.textContent = s ?? '';
  return d.innerHTML;
}

function render() {
  if (!state.data) return;
  renderSummary();
  renderItems();
}

function renderSummary() {
  const c = state.data.counts || {};
  const items = [
    { label: '进行中', num: c.active ?? 0, cls: '' },
    { label: '收件', num: c.inbox ?? 0, cls: c.inbox > 10 ? 'warn' : '' },
    { label: '待审', num: c.needs_review ?? 0, cls: c.needs_review > 0 ? 'warn' : '' },
    { label: '冲突', num: c.conflicts ?? 0, cls: c.conflicts > 0 ? 'danger' : '' },
    { label: 'WIP', num: `${c.active ?? 0}/${c.wip_limit ?? 0}`, cls: c.active >= c.wip_limit ? 'warn' : 'ok' },
  ];
  $('#summary').innerHTML = items.map(s =>
    `<div class="stat ${s.cls}"><span class="num">${s.num}</span><span class="label">${s.label}</span></div>`
  ).join('');
}

function renderItems() {
  const container = $('#items');
  const items = state.data.items || [];
  const captures = state.data.captures || [];
  const proposals = state.data.proposals || [];
  let html = '';

  if (state.view === 'inbox') {
    html += `<div class="capture">
      <input type="text" id="cap-text" placeholder="记录一个想法、问题或 Bug…" autocomplete="off">
      <select id="cap-kind"><option value="idea">想法</option><option value="question">问题</option><option value="bug">Bug</option><option value="request">需求</option></select>
      <button id="cap-btn">记录</button>
    </div>`;
  }

  for (const p of proposals) {
    html += `<div class="item" data-status="proposal" data-conflict="${p.state === 'conflict'}" data-id="${esc(p.id)}">
      <div class="bar"></div>
      <div class="content">
        <div class="title-row"><span class="title">整理建议</span><span class="id">${esc(p.id.slice(0,11))}</span></div>
        <div class="reason">${esc(p.reason)}</div>
        <div class="meta-row"><span>${p.state}</span></div>
      </div>
      <div class="actions">
        ${p.state === 'pending' ? `<button data-action="accept" data-id="${esc(p.id)}">接受</button><button data-action="reject" data-id="${esc(p.id)}">拒绝</button>` : ''}
      </div>
    </div>`;
  }

  for (const c of captures) {
    html += `<div class="item" data-status="inbox" data-id="${esc(c.id)}">
      <div class="bar"></div>
      <div class="content">
        <div class="title-row"><span class="title">${esc(c.text.slice(0,80))}</span><span class="id">${esc(c.kind)}</span></div>
        <div class="meta-row"><span>${c.source}</span><span class="sep">·</span><span>${new Date(c.at).toLocaleString()}</span></div>
      </div>
    </div>`;
  }

  for (const v of items) {
    const t = v.task;
    html += `<div class="item" data-status="${t.status}" data-conflict="${v.conflict}" data-id="${esc(t.id)}">
      <div class="bar"></div>
      <div class="content">
        <div class="title-row">
          <span class="title">${esc(t.title)}</span>
          <span class="id">${esc(t.id.slice(0,11))}</span>
          ${v.conflict ? '<span style="color:var(--danger);font-size:11px">冲突</span>' : ''}
        </div>
        <div class="reason">${esc(v.reason || t.next || t.summary || '—')}</div>
        <div class="meta-row">
          <span>${t.project}</span><span class="sep">·</span>
          <span>${t.status}</span><span class="sep">·</span>
          <span>P${t.priority}</span><span class="sep">·</span>
          <span>${v.machine}</span><span class="sep">·</span>
          <span>${new Date(v.updated_at).toLocaleString()}</span>
        </div>
      </div>
    </div>`;
  }

  if (!html) {
    html = `<div class="empty"><div class="icon">✦</div><p>这里没有需要关注的事项。</p></div>`;
  }

  container.innerHTML = html;
  bindItemEvents();
}

function bindItemEvents() {
  $$('#items .item').forEach(el => {
    el.addEventListener('click', e => {
      if (e.target.closest('button')) return;
      const id = el.dataset.id;
      if (id) showDetail(id);
    });
  });
  $$('#items .actions button').forEach(btn => {
    btn.addEventListener('click', async e => {
      e.stopPropagation();
      const id = btn.dataset.id;
      const action = btn.dataset.action;
      const token = sessionStorage.getItem('wm-token');
      try {
        const res = await fetch('/api/review', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${token}` },
          body: JSON.stringify({ id, accept: action === 'accept' }),
        });
        if (!res.ok) throw new Error(await res.text());
        load();
      } catch (err) { showError(err.message); }
    });
  });
  const capBtn = $('#cap-btn');
  if (capBtn) {
    capBtn.addEventListener('click', async () => {
      const text = $('#cap-text').value.trim();
      const kind = $('#cap-kind').value;
      if (!text) return;
      const token = sessionStorage.getItem('wm-token');
      try {
        const res = await fetch('/api/capture', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${token}` },
          body: JSON.stringify({ text, kind }),
        });
        if (!res.ok) throw new Error(await res.text());
        $('#cap-text').value = '';
        load();
      } catch (err) { showError(err.message); }
    });
    $('#cap-text').addEventListener('keydown', e => {
      if (e.key === 'Enter') capBtn.click();
    });
  }
}

function showDetail(id) {
  if (!state.data) return;
  const all = [
    ...(state.data.items || []).map(v => ({ type: 'direction', id: v.task.id, view: v, data: v.task })),
    ...(state.data.captures || []).map(c => ({ type: 'capture', id: c.id, data: c })),
  ];
  const found = all.find(x => x.id === id);
  if (!found) return;
  let html = '';

  if (found.type === 'capture') {
    const c = found.data;
    html = `<h2>原始记录</h2><div class="detail-id">${esc(c.id)}</div>
      <dl>
        <dt>类型</dt><dd>${c.kind}</dd>
        <dt>来源</dt><dd>${esc(c.source)}</dd>
        <dt>机器</dt><dd>${esc(c.machine)}</dd>
        <dt>时间</dt><dd>${new Date(c.at).toLocaleString()}</dd>
      </dl>
      <div class="original">${esc(c.text)}</div>`;
  } else {
    const v = found.view; const t = found.data;
    html = `<h2>${esc(t.title)}</h2><div class="detail-id">${esc(t.id)}</div>
      <dl>
        <dt>状态</dt><dd>${v.conflict ? '⚠ 冲突' : t.status}</dd>
        <dt>项目</dt><dd>${esc(t.project)}</dd>
        <dt>优先级</dt><dd>P${t.priority}</dd>
        <dt>目标</dt><dd>${esc(t.goal)}</dd>
        <dt>已完成</dt><dd>${esc(t.summary) || '—'}</dd>
        <dt>下一步</dt><dd>${esc(t.next) || '—'}</dd>
        <dt>阻塞</dt><dd>${esc(t.blocker) || '—'}</dd>
        <dt>最后报告</dt><dd>${new Date(v.updated_at).toLocaleString()} by ${esc(v.machine)}</dd>
        <dt>Git</dt><dd>${t.git_branch || '—'} @ ${t.git_head ? t.git_head.slice(0,8) : '—'}</dd>
        ${t.harness ? `<dt>Harness</dt><dd>${esc(t.harness)}</dd>` : ''}
      </dl>
      ${t.evidence.length ? `<div class="original" style="border-left-color:var(--ok)">验证: ${t.evidence.map(esc).join(' | ')}</div>` : ''}
      <div class="original" style="border-left-color:var(--warn);margin-top:8px">这是最后报告的状态，不是实时运行状态。证据为 Agent 报告，使用前需复核。</div>`;
  }

  $('#detail').innerHTML = html;
  $('#overlay').hidden = false;
}

/* ── Event wiring ───────────────────────── */
$$('.views button').forEach(btn => {
  btn.addEventListener('click', () => {
    $$('.views button').forEach(b => b.classList.remove('active'));
    btn.classList.add('active');
    state.view = btn.dataset.view;
    load();
  });
});

$('#q').addEventListener('input', e => {
  state.search = e.target.value;
  clearTimeout(state._searchTimer);
  state._searchTimer = setTimeout(load, 300);
});

$('#refresh').addEventListener('click', load);
$('#overlay').addEventListener('click', e => {
  if (e.target === e.currentTarget) $('#overlay').hidden = true;
});
document.addEventListener('keydown', e => {
  if (e.key === 'Escape') $('#overlay').hidden = true;
});

load();
