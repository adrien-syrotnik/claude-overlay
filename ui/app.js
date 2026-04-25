const { listen } = window.__TAURI__.event;
const { invoke } = window.__TAURI__.core;

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
  if (state.yesno_format) {
    const y = document.createElement('button'); y.className = 'btn btn-accent'; y.textContent = 'Yes';
    y.onclick = () => invoke('notif_send_yes', { id: state.id });
    const n = document.createElement('button'); n.className = 'btn btn-accent no'; n.textContent = 'No';
    n.onclick = () => invoke('notif_send_no', { id: state.id });
    group.append(y, n);
  }
  const f = document.createElement('button'); f.className = 'btn'; f.textContent = 'Focus';
  f.onclick = () => invoke('notif_focus', { id: state.id });
  const x = document.createElement('button'); x.className = 'btn btn-icon'; x.textContent = '×';
  x.onclick = () => invoke('notif_dismiss', { id: state.id });
  group.append(f, x);
  li.appendChild(group);

  return li;
}

function addRow(state) {
  document.getElementById('pill').classList.remove('pill-hidden');
  document.getElementById('notif-list').appendChild(mkRow(state));
}

function removeRow(id) {
  const row = document.querySelector(`.notif-row[data-id="${id}"]`);
  if (!row) return;
  row.classList.add('leaving');
  setTimeout(() => {
    row.remove();
    if (!document.querySelector('.notif-row')) {
      document.getElementById('pill').classList.add('pill-hidden');
    }
  }, 200);
}

(async () => {
  // Restore existing notifs on window reload
  const list = await invoke('notif_list');
  list.forEach(addRow);

  await listen('notif:new', e => addRow(e.payload));
  await listen('notif:remove', e => removeRow(e.payload));
})();
