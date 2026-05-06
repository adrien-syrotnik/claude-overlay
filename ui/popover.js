function showError(msg) {
  const root = document.getElementById('popover-content') || document.body;
  while (root.firstChild) root.removeChild(root.firstChild);
  const p = document.createElement('div');
  p.className = 'popover-error';
  p.textContent = msg;
  root.appendChild(p);
  try {
    const api = getApi();
    if (api) {
      requestAnimationFrame(() => {
        const h = root.getBoundingClientRect().height + 16;
        api.invoke('set_popover_height_px', { height: Math.ceil(h) });
      });
    }
  } catch (_) {}
}

let notifId = null;
let isMulti = false;
let allowOther = false;
const selected = new Set();

function getApi() {
  const t = window.__TAURI__;
  if (!t || !t.event || !t.core) return null;
  return { listen: t.event.listen, invoke: t.core.invoke };
}

function clearChildren(el) {
  while (el.firstChild) el.removeChild(el.firstChild);
}

function applyData(invoke, data) {
  if (!data) { showError('payload empty'); return; }
  if (!Array.isArray(data.items) || data.items.length === 0) {
    showError('no items');
    return;
  }
  notifId = data.notif_id;
  isMulti = !!data.multi_select;
  allowOther = !!data.allow_other;
  selected.clear();
  render(invoke, data.items);
}

function render(invoke, items) {
  const root = document.getElementById('popover-content');
  clearChildren(root);

  items.forEach(item => {
    if (isMulti) {
      const lbl = document.createElement('label');
      lbl.className = 'popover-item popover-checkbox';
      const cb = document.createElement('input');
      cb.type = 'checkbox';
      cb.onchange = () => cb.checked ? selected.add(item.label) : selected.delete(item.label);
      const text = document.createElement('span');
      text.className = 'popover-label';
      text.textContent = item.label;
      lbl.append(cb, text);
      if (item.description) {
        const d = document.createElement('span');
        d.className = 'popover-desc';
        d.textContent = item.description;
        lbl.append(d);
      }
      root.appendChild(lbl);
    } else {
      const btn = document.createElement('button');
      btn.className = 'popover-item';
      const text = document.createElement('span');
      text.className = 'popover-label';
      text.textContent = item.label;
      btn.append(text);
      if (item.description) {
        const d = document.createElement('span');
        d.className = 'popover-desc';
        d.textContent = item.description;
        btn.append(d);
      }
      btn.onclick = () => {
        invoke('notif_answer', { id: notifId, answer: item.label });
        invoke('close_popover');
      };
      root.appendChild(btn);
    }
  });

  if (allowOther && !isMulti) {
    const other = document.createElement('button');
    other.className = 'popover-item popover-other';
    other.textContent = 'Other…';
    other.onclick = () => invoke('close_popover');
    root.appendChild(other);
  }

  if (isMulti) {
    const submit = document.createElement('button');
    submit.className = 'popover-item popover-submit';
    submit.textContent = 'Submit';
    submit.onclick = () => {
      invoke('notif_answer_multi', { id: notifId, answers: Array.from(selected) });
      invoke('close_popover');
    };
    root.appendChild(submit);
  }

  requestAnimationFrame(() => {
    const h = root.getBoundingClientRect().height + 8;
    invoke('set_popover_height_px', { height: Math.ceil(h) });
  });
}

function bindGlobalHandlers(api) {
  window.addEventListener('blur', () => api.invoke('close_popover'));
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      api.invoke('close_popover');
    } else if (e.key === 'Enter' && isMulti) {
      e.preventDefault();
      api.invoke('notif_answer_multi', { id: notifId, answers: Array.from(selected) });
      api.invoke('close_popover');
    }
  });
  api.listen('popover:show', (e) => {
    if (e && e.payload && Array.isArray(e.payload.items)) {
      applyData(api.invoke, e.payload);
    } else {
      api.invoke('get_popover_data').then(d => applyData(api.invoke, d)).catch(err => {
        showError('listen fallback err: ' + err);
      });
    }
  });
}

function init() {
  const api = getApi();
  if (!api) {
    setTimeout(init, 30);
    return;
  }
  bindGlobalHandlers(api);
  api.invoke('get_popover_data').then(d => {
    if (d) applyData(api.invoke, d);
  }).catch(() => {});
}

if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', init);
} else {
  init();
}
