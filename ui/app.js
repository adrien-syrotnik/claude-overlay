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
    if (state.options && state.options.length > 0) {
      // AskUserQuestion: one button per option. Click sends the option text
      // back to the waiting hook, which emits PreToolUse decision:block.
      state.options.forEach(opt => {
        const b = document.createElement('button');
        b.className = 'btn btn-accent';
        b.textContent = opt;
        b.title = opt;
        b.onclick = () => invoke('notif_answer', { id: state.id, answer: opt });
        group.append(b);
      });
    } else if (state.yesno_format) {
      // permission_prompt is Claude Code's native picker — label buttons
      // Allow/Deny since "Yes/No" is misleading for the 3-option case where
      // option 2 is "Yes, and don't ask again". Allow always sends "1" (Yes),
      // Deny sends Esc (cancels the picker = denial).
      const isPerm = state.notification_type === 'permission_prompt';
      const y = document.createElement('button'); y.className = 'btn btn-accent';
      y.textContent = isPerm ? 'Allow' : 'Yes';
      y.onclick = () => invoke('notif_send_yes', { id: state.id });
      const n = document.createElement('button'); n.className = 'btn btn-accent no';
      n.textContent = isPerm ? 'Deny' : 'No';
      n.onclick = () => invoke('notif_send_no', { id: state.id });
      group.append(y, n);
    }
    if (!state.options || state.options.length === 0) {
      const f = document.createElement('button'); f.className = 'btn'; f.textContent = 'Focus';
      f.onclick = () => invoke('notif_focus', { id: state.id });
      group.append(f);
    }
    const x = document.createElement('button'); x.className = 'btn btn-icon'; x.textContent = '×';
    x.onclick = () => invoke('notif_dismiss', { id: state.id });
    group.append(x);
    li.appendChild(group);

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

    const visibleRowCount = visible.length + (overflowCount > 0 ? 1 : 0);
    // TODO Task 9: pass real denseRows and popoverOpen.
    invoke('set_overlay_height', { rows: visibleRowCount, denseRows: 0, popoverOpen: false });
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
