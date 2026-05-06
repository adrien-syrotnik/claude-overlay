const tauri = window.__TAURI__;
const statusEl = document.getElementById('status-text');
const dotEl = document.querySelector('.status-bar .dot');
const listEl = document.getElementById('notif-list');

const MAX_VISIBLE = 3;
const states = new Map();

function setStatus(text, kind) {
  statusEl.textContent = text;
  dotEl.className = 'dot ' + (kind || 'dot-idle');
}

function refreshStatus(count) {
  if (count > 0) setStatus(`claude-overlay · ${count} pending`, 'dot-active');
  else setStatus('claude-overlay · idle', 'dot-idle');
}

if (!tauri || !tauri.event || !tauri.core) {
  setStatus('claude-overlay · JS bridge missing', 'dot-stop');
  console.error('window.__TAURI__ unavailable');
} else {
  const { listen } = tauri.event;
  const { invoke } = tauri.core;

  const POPOVER_OPTIONS_THRESHOLD = 3;
  const POPOVER_LABEL_THRESHOLD = 18;

  function shouldUsePopover(options) {
    if (options.length > POPOVER_OPTIONS_THRESHOLD) return true;
    return options.some(o => (o.label || '').length > POPOVER_LABEL_THRESHOLD);
  }

  function mkButton(label, opts = {}) {
    const b = document.createElement('button');
    b.className = 'btn' + (opts.accent ? ' btn-accent' : '') + (opts.icon ? ' btn-icon' : '');
    if (opts.chevron) {
      const lbl = document.createElement('span');
      lbl.className = 'btn-label';
      lbl.textContent = label;
      const chev = document.createElement('span');
      chev.className = 'btn-chevron';
      chev.textContent = '▾'; // ▾
      chev.setAttribute('aria-hidden', 'true');
      b.append(lbl, chev);
      b.classList.add('btn-with-chevron');
    } else {
      b.textContent = label;
    }
    if (opts.title) b.title = opts.title;
    if (opts.onClick) b.onclick = opts.onClick;
    return b;
  }

  function mkFocusBtn(state) {
    return mkButton('Focus', { onClick: () => invoke('notif_focus', { id: state.id }) });
  }

  function mkDismissBtn(state) {
    return mkButton('×', { icon: true, onClick: () => invoke('notif_dismiss', { id: state.id }) });
  }

  function renderNone(state, group) {
    group.append(mkFocusBtn(state), mkDismissBtn(state));
  }

  function renderYesNo(state, group) {
    const isPerm = state.notification_type === 'permission_prompt';
    const yes = mkButton(isPerm ? 'Allow' : 'Yes', { accent: true,
      onClick: () => invoke('notif_yes_no', { id: state.id, choice: true }) });
    const no = mkButton(isPerm ? 'Deny' : 'No',
      { onClick: () => invoke('notif_yes_no', { id: state.id, choice: false }) });
    group.append(yes, no, mkDismissBtn(state));
  }

  function renderSingleChoice(state, group, row) {
    const { options, allow_other } = state.input;
    if (shouldUsePopover(options)) {
      const trigger = mkButton('Choose', { accent: true, chevron: true,
        onClick: (e) => openExternalPopover(state, options, allow_other, false, e.currentTarget) });
      group.append(trigger);
    } else {
      options.forEach(opt => {
        const b = mkButton(opt.label, { accent: true, title: opt.description || opt.label,
          onClick: () => invoke('notif_answer', { id: state.id, answer: opt.label }) });
        group.append(b);
      });
      if (allow_other) {
        group.append(mkButton('Other…', { onClick: () => switchToText(row, state) }));
      }
    }
    group.append(mkDismissBtn(state));
  }

  function renderMultiChoice(state, group, row) {
    const { options, allow_other } = state.input;
    const selected = new Set();
    if (shouldUsePopover(options)) {
      const trigger = mkButton('Select…', { accent: true, chevron: true,
        onClick: (e) => openExternalPopover(state, options, allow_other, true, e.currentTarget) });
      group.append(trigger);
    } else {
      const list = document.createElement('span');
      list.className = 'checkbox-list';
      options.forEach(opt => {
        const lbl = document.createElement('label');
        lbl.className = 'cb';
        const cb = document.createElement('input');
        cb.type = 'checkbox';
        cb.onchange = () => cb.checked ? selected.add(opt.label) : selected.delete(opt.label);
        lbl.append(cb, document.createTextNode(opt.label));
        lbl.title = opt.description || opt.label;
        list.append(lbl);
      });
      group.append(list);
      if (allow_other) {
        group.append(mkButton('Other…', { onClick: () => switchToText(row, state) }));
      }
      group.append(mkButton('Submit', { accent: true,
        onClick: () => invoke('notif_answer_multi', { id: state.id, answers: Array.from(selected) }) }));
    }
    group.append(mkDismissBtn(state));
  }

  function renderTextInput(state, group, row) {
    const input = document.createElement('input');
    input.type = 'text';
    input.className = 'text-input';
    input.placeholder = (state.input && state.input.placeholder) || 'Type your answer…';
    input.onkeydown = (e) => {
      if (e.key === 'Enter') {
        e.preventDefault();
        submitText(state.id, input.value);
      } else if (e.key === 'Escape') {
        e.preventDefault();
        invoke('notif_dismiss', { id: state.id });
      }
    };
    group.append(input);
    group.append(mkButton('Submit', { accent: true, onClick: () => submitText(state.id, input.value) }));
    group.append(mkDismissBtn(state));
    setTimeout(() => input.focus(), 0);
  }

  function submitText(id, text) {
    if (text.length === 0) return;
    invoke('notif_text', { id, text });
  }

  function switchToText(row, state) {
    const oldGroup = row.querySelector('.btn-group');
    if (oldGroup) oldGroup.remove();
    const newGroup = document.createElement('span');
    newGroup.className = 'btn-group';
    const fakeState = Object.assign({}, state, { input: { kind: 'text_input', placeholder: 'Other…' } });
    renderTextInput(fakeState, newGroup, row);
    row.appendChild(newGroup);
  }

  function syncOverlayHeight() {
    const pill = document.querySelector('.pill');
    if (!pill) return;
    const h = pill.getBoundingClientRect().height;
    invoke('set_overlay_height_px', { height: Math.ceil(h) });
  }

  function openExternalPopover(state, options, allowOther, multiSelect, triggerEl) {
    const r = triggerEl.getBoundingClientRect();
    invoke('open_popover', {
      notifId: state.id,
      items: options,
      multiSelect: !!multiSelect,
      allowOther: !!allowOther,
      anchorX: r.left,
      anchorY: r.top,
      anchorHeight: r.height,
    }).catch(err => console.error('open_popover failed', err));
  }

  function applyDenseClass(row, state) {
    const msgLen = (state.message || '').length;
    const optsLen = (state.input && state.input.options || [])
      .reduce((s, o) => s + (o.label || '').length, 0);
    if (msgLen + optsLen > 80) row.classList.add('dense');
  }

  function mkRow(state) {
    const li = document.createElement('li');
    li.className = 'notif-row';
    li.dataset.id = state.id;

    const dot = document.createElement('span');
    dot.className = `status-dot ${state.event === 'Stop' ? 'stop' : 'notification'}`;
    li.appendChild(dot);

    const bn = document.createElement('span');
    bn.className = 'basename'; bn.textContent = state.source_basename;
    li.appendChild(bn);

    const sep = document.createElement('span');
    sep.className = 'separator'; sep.textContent = '›';
    li.appendChild(sep);

    const msg = document.createElement('span');
    msg.className = 'message'; msg.textContent = state.message; msg.title = state.message;
    li.appendChild(msg);

    const group = document.createElement('span');
    group.className = 'btn-group';

    const kind = state.input && state.input.kind || 'none';
    switch (kind) {
      case 'yes_no':        renderYesNo(state, group); break;
      case 'single_choice': renderSingleChoice(state, group, li); break;
      case 'multi_choice':  renderMultiChoice(state, group, li); break;
      case 'text_input':    renderTextInput(state, group, li); break;
      default:              renderNone(state, group);
    }
    li.appendChild(group);
    applyDenseClass(li, state);
    return li;
  }

  function reconcile() {
    // Order by created order (Map preserves insertion order). Newest = last.
    const all = Array.from(states.values());
    const total = all.length;
    const visible = all.slice(-MAX_VISIBLE);
    const visibleSet = new Set(visible.map(s => s.id));

    // Remove rows no longer visible (with leave animation)
    Array.from(listEl.querySelectorAll('.notif-row')).forEach(row => {
      if (!visibleSet.has(row.dataset.id)) {
        if (!row.classList.contains('leaving')) {
          row.classList.add('leaving');
          setTimeout(() => row.remove(), 200);
        }
      }
    });

    // Add new rows in correct order
    visible.forEach((s, i) => {
      const existing = listEl.querySelector(`.notif-row[data-id="${s.id}"]`);
      if (!existing) {
        const li = mkRow(s);
        // Insert at correct position relative to siblings
        const refRow = listEl.querySelectorAll('.notif-row')[i];
        if (refRow) listEl.insertBefore(li, refRow); else listEl.appendChild(li);
      }
    });

    // Overflow indicator
    const overflowCount = total - visible.length;
    let overflow = listEl.querySelector('.notif-overflow');
    if (overflowCount > 0) {
      if (!overflow) {
        overflow = document.createElement('li');
        overflow.className = 'notif-overflow';
        listEl.appendChild(overflow);
      } else {
        listEl.appendChild(overflow); // ensure last
      }
      overflow.textContent = `+ ${overflowCount} more`;
    } else if (overflow) {
      overflow.remove();
    }

    refreshStatus(total);

    requestAnimationFrame(syncOverlayHeight);
  }

  function addNotif(state) {
    states.delete(state.id); // ensure newest insertion order if same id
    states.set(state.id, state);
    reconcile();
  }

  function removeNotif(id) {
    states.delete(id);
    reconcile();
  }

  (async () => {
    try {
      const list = await invoke('notif_list');
      list.forEach(s => states.set(s.id, s));
      reconcile();
      await listen('notif:new', e => addNotif(e.payload));
      await listen('notif:remove', e => removeNotif(e.payload));
      console.log('claude-overlay UI ready');
    } catch (err) {
      setStatus('claude-overlay · init error', 'dot-stop');
      console.error('init failed', err);
    }
  })();
}
