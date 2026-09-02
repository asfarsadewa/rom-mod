'use strict';

const PLATFORM_LABEL = { nes: 'NES', snes: 'Super NES', genesis: 'Mega Drive' };
const CORES = {
  nes: ['Mesen', 'Nestopia', 'FCEUmm', 'QuickNES'],
  snes: ['Mesen-S', 'Snes9x', 'bsnes'],
  genesis: ['Genesis Plus GX', 'PicoDrive', 'BlastEm'],
};

const state = {
  version: '',
  roots: [],
  retroarch: null,
  library: [],
  filter: '',
  romId: null,
  rom: null,
  cheats: null,
  custom: [],
  customText: '',
  label: '',
  selected: new Map(),
  result: null,
  busy: false,
};

const $ = (sel, root = document) => root.querySelector(sel);
const esc = s => String(s ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const hex = (n, w) => Number(n).toString(16).toUpperCase().padStart(w, '0');
const hexBytes = arr => arr.map(b => hex(b, 2)).join(' ');
const fmtSize = n => (n >= 1048576 ? `${(n / 1048576).toFixed(n % 1048576 ? 2 : 0)} MB` : `${Math.round(n / 1024)} KB`);

async function api(path, body) {
  const init = body === undefined
    ? {}
    : { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) };
  const r = await fetch(path, init);
  let j;
  try { j = await r.json(); } catch { j = { error: `${r.status} ${r.statusText}` }; }
  if (!r.ok) throw new Error(j.error || `${r.status}`);
  return j;
}

let toastTimer = null;
function toast(msg, kind = '') {
  const el = $('#toast');
  el.textContent = msg;
  el.className = `toast ${kind}`;
  el.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { el.hidden = true; }, 3800);
}

function setStatus(text) { $('#bar-status').textContent = text; }
function statusLine() { return `${state.library.length} ROMs · v${state.version}`; }

/* ---------- library ---------- */

function renderLibrary() {
  const q = state.filter.trim().toLowerCase();
  const items = state.library.filter(e => !q || e.name.toLowerCase().includes(q));
  const groups = new Map();
  for (const e of items) {
    if (!groups.has(e.platform)) groups.set(e.platform, []);
    groups.get(e.platform).push(e);
  }
  let html = '';
  for (const [p, list] of groups) {
    html += `<div class="group"><div class="group-head"><span class="dot ${p}"></span>${esc(PLATFORM_LABEL[p] || p)}<span class="group-n">${list.length}</span></div>`;
    for (const e of list) {
      html += `<button class="item${e.id === state.romId ? ' active' : ''}" data-id="${e.id}" title="${esc(e.path)}"><span class="item-name">${esc(e.name)}</span><span class="item-size">${fmtSize(e.bytes)}</span></button>`;
    }
    html += '</div>';
  }
  if (!html) html = `<div class="rail-empty">${state.library.length ? 'No matches.' : 'Scan a folder to begin.'}</div>`;
  $('#library').innerHTML = html;
  $('#count').textContent = state.library.length ? `${items.length} / ${state.library.length}` : '';
}

async function loadState() {
  const s = await api('/api/state');
  Object.assign(state, { version: s.version, roots: s.roots, retroarch: s.retroarch_dir, library: s.library });
  if (!$('#root').value) $('#root').value = s.roots[0] || '';
  renderLibrary();
  setStatus(statusLine());
}

async function scan(root) {
  setStatus('Scanning…');
  try {
    const s = await api('/api/scan', { root });
    Object.assign(state, { roots: s.roots, retroarch: s.retroarch_dir, library: s.library });
    renderLibrary();
    setStatus(statusLine());
  } catch (e) {
    toast(e.message, 'bad');
    setStatus(statusLine());
  }
}

/* ---------- stage ---------- */

async function selectRom(id) {
  if (state.busy || id === state.romId) return;
  Object.assign(state, { romId: id, rom: null, cheats: null, custom: [], customText: '', result: null });
  state.selected.clear();
  renderLibrary();
  $('#stage').innerHTML = '<div class="empty"><p class="empty-title">Reading…</p></div>';
  try {
    state.rom = await api(`/api/rom/${id}`);
    renderStage();
    state.cheats = await api(`/api/rom/${id}/cheats`);
    if (state.romId === id) renderStage();
  } catch (e) {
    $('#stage').innerHTML = `<div class="empty"><p class="empty-title">Could not read this ROM</p><p class="empty-sub">${esc(e.message)}</p></div>`;
  }
}

async function pickCandidate(name) {
  state.cheats = { ...state.cheats, cheats: [], loading: true };
  for (const k of [...state.selected.keys()]) if (k.startsWith('db:')) state.selected.delete(k);
  renderStage();
  try {
    state.cheats = await api(`/api/rom/${state.romId}/cheats?name=${encodeURIComponent(name)}`);
  } catch (e) {
    toast(e.message, 'bad');
    state.cheats = { ...state.cheats, loading: false };
  }
  renderStage();
}

function kindOf(c) {
  if (c.broken) return ['broken', 'Unreadable'];
  if (c.runtime) return ['runtime', 'Runtime'];
  if (c.noop) return ['noop', 'Already in ROM'];
  return ['patch', 'ROM patch'];
}

function summary(c) {
  const bits = [];
  for (const p of c.parts) {
    if (p.error) { bits.push('—'); continue; }
    if (p.op?.kind === 'ram') {
      bits.push(`RAM $${hex(p.op.addr, 6)} = $${hex(p.op.value, p.op.width * 2)}`);
    } else if (p.rom_ops.length) {
      for (const o of p.rom_ops.slice(0, 2)) bits.push(`$${hex(o.offset, 6)}: ${hexBytes(o.old)} → ${hexBytes(o.new)}`);
      if (p.rom_ops.length > 2) bits.push(`+${p.rom_ops.length - 2} more`);
    } else if (p.notes.length) {
      bits.push(p.notes[0]);
    }
  }
  return bits.join('  ·  ');
}

function renderRow(c, i, source) {
  const [kind, label] = kindOf(c);
  const key = `${source}:${i}`;
  const checked = state.selected.has(key) ? ' checked' : '';
  const disabled = c.broken || c.noop ? ' disabled' : '';
  return `
  <div class="row ${kind}" data-key="${esc(key)}">
    <label class="check"><input type="checkbox"${checked}${disabled} data-key="${esc(key)}"></label>
    <span class="desc" title="${esc(c.desc)}">${esc(c.desc)}</span>
    <span class="code" title="${esc(c.code)}">${esc(c.code)}</span>
    <span class="type"><span class="pill ${kind}">${label}</span></span>
    <span class="effect" title="${esc(summary(c))}">${esc(summary(c))}</span>
  </div>
  <div class="details" data-for="${esc(key)}" hidden>${renderDetails(c)}</div>`;
}

function renderDetails(c) {
  return c.parts.map(p => {
    let head = `<span class="code">${esc(p.raw)}</span><span class="fmt">${esc(p.format)}</span>`;
    if (p.op?.kind === 'rom') {
      const w = p.op.cpu_addr > 0xFFFF ? 6 : 4;
      head += `<span class="addr">$${hex(p.op.cpu_addr, w)} ← $${hex(p.op.value, p.op.width * 2)}${p.op.compare != null ? ` if $${hex(p.op.compare, 2)}` : ''}</span>`;
    }
    if (p.op?.kind === 'ram') {
      head += `<span class="addr">RAM $${hex(p.op.addr, 6)} ← $${hex(p.op.value, p.op.width * 2)}</span>`;
    }
    let body = '';
    if (p.rom_ops.length) {
      body += `<table class="ops"><thead><tr><th>File offset</th><th>Was</th><th>Becomes</th></tr></thead><tbody>${p.rom_ops.map(o => `<tr><td>$${hex(o.offset, 6)}</td><td>${hexBytes(o.old)}</td><td>${hexBytes(o.new)}</td></tr>`).join('')}</tbody></table>`;
    }
    if (p.notes.length) body += `<ul class="notes">${p.notes.map(n => `<li>${esc(n)}</li>`).join('')}</ul>`;
    if (p.error) body += `<p class="err">${esc(p.error)}</p>`;
    return `<div class="part"><div class="part-head">${head}</div>${body}</div>`;
  }).join('');
}

function renderResult() {
  const r = state.result;
  if (!r) return '';
  if (r.kind === 'cht') {
    return `<section class="panel result"><header class="panel-head"><h2>Cheat file written</h2></header>
      <div class="kv"><span>File</span><span class="mono">${esc(r.path)}</span></div>
      <span class="hint">In RetroArch: Quick Menu → Cheats → Load Cheat File. Or turn on “Apply Cheats After Load” once and it is picked up automatically.</span></section>`;
  }
  const ck = r.checksum ? ` · checksum ${r.checksum[0]} → ${r.checksum[1]}` : '';
  return `<section class="panel result"><header class="panel-head"><h2>Built</h2></header>
    <div class="kv"><span>ROM</span><span class="mono">${esc(r.rom_path)}</span></div>
    <div class="kv"><span>IPS</span><span class="mono">${esc(r.ips_path)}</span></div>
    <div class="kv"><span>Changed</span><span>${r.changed_bytes} byte${r.changed_bytes === 1 ? '' : 's'}${ck}</span></div>
    <div class="kv"><span>SHA-1</span><span class="mono">${esc(r.sha1)}</span></div>
    <table class="ops"><thead><tr><th>File offset</th><th>Was</th><th>Becomes</th></tr></thead><tbody>${r.ops.map(o => `<tr><td>$${hex(o.offset, 6)}</td><td>${hexBytes(o.old)}</td><td>${hexBytes(o.new)}</td></tr>`).join('')}</tbody></table></section>`;
}

function renderStage() {
  const r = state.rom;
  if (!r) return;
  const info = r.info;
  const ck = info.checksum
    ? `<span class="${info.checksum.valid ? 'ok' : 'bad'}">checksum ${info.checksum.valid ? 'valid' : `${info.checksum.stored} ≠ ${info.checksum.computed}`}</span>`
    : '';
  const meta = [
    info.title && info.title !== r.name ? esc(info.title) : '',
    esc(info.region),
    fmtSize(info.size),
    `<span class="mono">sha1 ${info.sha1.slice(0, 12)}</span>`,
    ck,
  ].filter(Boolean).join('<span class="sep">·</span>');
  const facts = info.fields.map(f => `<div class="fact"><dt>${esc(f.label)}</dt><dd>${esc(f.value)}</dd></div>`).join('');
  const notes = info.notes.map(n => `<p class="note">${esc(n)}</p>`).join('');

  const c = state.cheats;
  let cheatsBody = '';
  if (!c) cheatsBody = '<div class="panel-empty">Looking up the cheat database…</div>';
  else if (c.loading) cheatsBody = '<div class="panel-empty">Fetching…</div>';
  else if (c.cheats.length) {
    cheatsBody = `<div class="table"><div class="row head"><span></span><span>Cheat</span><span>Code</span><span>Type</span><span>Effect on this file</span></div>${c.cheats.map((x, i) => renderRow(x, i, 'db')).join('')}</div>`;
  } else if (c.error) cheatsBody = `<div class="panel-empty">${esc(c.error)}</div>`;
  else if (!c.matched) cheatsBody = `<div class="panel-empty">${c.candidates.length ? 'No exact match. Pick the closest entry above.' : 'Nothing in the database under this name. Paste codes below.'}</div>`;
  else cheatsBody = '<div class="panel-empty">The database entry has no usable codes.</div>';

  let source = '';
  if (c && !c.loading) {
    const wantsPicker = c.candidates.length > 1 || (!c.matched && c.candidates.length);
    if (wantsPicker) {
      const opts = c.candidates.map(n => `<option value="${esc(n)}"${n === c.matched ? ' selected' : ''}>${esc(n)}</option>`).join('');
      source = `<label class="source">${esc(c.source)} <select id="candidate"><option value="">choose an entry…</option>${opts}</select></label>`;
    } else {
      source = `<span class="source">${esc(c.source)}${c.matched ? ` · ${esc(c.matched)}` : ''}</span>`;
    }
  }

  const customRows = state.custom.length ? `<div class="table">${state.custom.map((x, i) => renderRow(x, i, 'me')).join('')}</div>` : '';
  const cores = CORES[r.platform] || [];

  $('#stage').innerHTML = `
    <section class="rom-head">
      <div class="title-row"><span class="chip ${r.platform}">${esc(r.platform_label)}</span><h1>${esc(r.name)}</h1></div>
      <div class="meta">${meta}</div>
      <dl class="facts">${facts}</dl>
      ${notes}
    </section>
    <section class="panel">
      <header class="panel-head"><h2>Cheats</h2>${source}</header>
      ${cheatsBody}
    </section>
    <section class="panel">
      <header class="panel-head"><h2>Your codes</h2><span class="hint">One per line. A label goes before an equals sign.</span></header>
      ${customRows}
      <div class="custom"><textarea id="custom" rows="3" spellcheck="false" placeholder="Infinite lives = SXIOPO">${esc(state.customText)}</textarea><button class="btn" id="decode">Decode</button></div>
    </section>
    ${renderResult()}
    <footer class="buildbar">
      <div class="build-sel" id="selection"></div>
      <div class="build-actions">
        <input id="label" type="text" spellcheck="false" placeholder="Label for the file name" value="${esc(state.label)}">
        <button class="btn primary" id="build">Build patched ROM</button>
        <select id="core">${cores.map(x => `<option>${esc(x)}</option>`).join('')}</select>
        <button class="btn" id="cht">Write RetroArch cheats</button>
      </div>
    </footer>`;
  updateSelection();
}

function lookup(key) {
  const [src, i] = key.split(':');
  const list = src === 'db' ? state.cheats?.cheats : state.custom;
  return list?.[Number(i)];
}

function updateSelection() {
  const el = $('#selection');
  if (!el) return;
  const sel = [...state.selected.values()];
  const patch = sel.filter(c => c.patchable && !c.noop);
  const runtime = sel.filter(c => c.runtime);
  el.textContent = sel.length
    ? `${sel.length} selected · ${patch.length} patch · ${runtime.length} runtime`
    : 'Select cheats to build';
  $('#build').disabled = !patch.length || state.busy;
  $('#cht').disabled = !sel.length || state.busy;
}

async function decodeCustom() {
  const text = $('#custom').value;
  state.customText = text;
  const lines = text.split('\n').map(s => s.trim()).filter(Boolean);
  if (!lines.length) return;
  for (const k of [...state.selected.keys()]) if (k.startsWith('me:')) state.selected.delete(k);
  try {
    state.custom = await api(`/api/rom/${state.romId}/decode`, { codes: lines });
    renderStage();
  } catch (e) {
    toast(e.message, 'bad');
  }
}

async function build(overwrite = false) {
  const sel = [...state.selected.values()].filter(c => c.patchable && !c.noop);
  if (!sel.length) return;
  const label = $('#label').value.trim() || (sel.length === 1 ? sel[0].desc : 'Modded');
  state.busy = true;
  updateSelection();
  try {
    const res = await api(`/api/rom/${state.romId}/build`, { label, codes: sel.map(c => c.code), overwrite });
    state.result = { kind: 'rom', ...res };
    toast('Patched ROM written', 'ok');
    renderStage();
    $('#stage .result')?.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
  } catch (e) {
    if (e.message.startsWith('EXISTS:')) {
      state.busy = false;
      if (confirm(`Overwrite ${e.message.slice(7)}?`)) return build(true);
    } else {
      toast(e.message, 'bad');
    }
  } finally {
    state.busy = false;
    updateSelection();
  }
}

async function writeCht() {
  const sel = [...state.selected.values()];
  if (!sel.length) return;
  let dir = state.retroarch;
  if (!dir) {
    dir = prompt('RetroArch folder (the one that holds retroarch.cfg)');
    if (!dir) return;
  }
  state.busy = true;
  updateSelection();
  try {
    const r = await api(`/api/rom/${state.romId}/cht`, {
      core: $('#core').value,
      cheats: sel.map(c => ({ desc: c.desc, code: c.code })),
      retroarch_dir: dir,
    });
    state.retroarch = dir;
    state.result = { kind: 'cht', ...r };
    toast('Cheat file written', 'ok');
    renderStage();
    $('#stage .result')?.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
  } catch (e) {
    toast(e.message, 'bad');
  } finally {
    state.busy = false;
    updateSelection();
  }
}

/* ---------- events ---------- */

document.addEventListener('DOMContentLoaded', () => {
  loadState().catch(e => toast(e.message, 'bad'));
  $('#scan').addEventListener('click', () => scan($('#root').value));
  $('#root').addEventListener('keydown', ev => {
    if (ev.key === 'Enter') { ev.preventDefault(); scan(ev.target.value); }
  });
  $('#filter').addEventListener('input', ev => { state.filter = ev.target.value; renderLibrary(); });
  $('#library').addEventListener('click', ev => {
    const b = ev.target.closest('.item');
    if (b) selectRom(b.dataset.id);
  });

  const stage = $('#stage');
  stage.addEventListener('change', ev => {
    const t = ev.target;
    if (t.matches('input[type=checkbox][data-key]')) {
      const c = lookup(t.dataset.key);
      if (!c) return;
      if (t.checked) state.selected.set(t.dataset.key, c); else state.selected.delete(t.dataset.key);
      updateSelection();
    } else if (t.id === 'candidate' && t.value) {
      pickCandidate(t.value);
    }
  });
  stage.addEventListener('input', ev => {
    if (ev.target.id === 'label') state.label = ev.target.value;
    if (ev.target.id === 'custom') state.customText = ev.target.value;
  });
  stage.addEventListener('click', ev => {
    const t = ev.target;
    if (t.closest('#build')) return build();
    if (t.closest('#cht')) return writeCht();
    if (t.closest('#decode')) return decodeCustom();
    const row = t.closest('.row[data-key]');
    if (row && !t.closest('.check')) {
      const d = stage.querySelector(`.details[data-for="${CSS.escape(row.dataset.key)}"]`);
      if (d) { d.hidden = !d.hidden; row.classList.toggle('open', !d.hidden); }
    }
  });
  stage.addEventListener('keydown', ev => {
    if (ev.target.id === 'custom' && ev.key === 'Enter' && (ev.ctrlKey || ev.metaKey)) {
      ev.preventDefault();
      decodeCustom();
    }
  });
});
