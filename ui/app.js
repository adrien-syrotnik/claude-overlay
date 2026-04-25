// Temporary: inject a fake notif on load so we can see the overlay.
// Will be replaced with Tauri event listeners in Task 10.

function mkRow({ id, event, basename, message, yesno, source_type }) {
  const li = document.createElement('li');
  li.className = 'notif-row';
  li.dataset.id = id;

  const dot = document.createElement('span');
  dot.className = `status-dot ${event === 'Stop' ? 'stop' : 'notification'}`;
  li.appendChild(dot);

  const bn = document.createElement('span');
  bn.className = 'basename';
  bn.textContent = basename;
  li.appendChild(bn);

  const sep = document.createElement('span');
  sep.className = 'separator';
  sep.textContent = '›';
  li.appendChild(sep);

  const msg = document.createElement('span');
  msg.className = 'message';
  msg.textContent = message;
  msg.title = message;
  li.appendChild(msg);

  const group = document.createElement('span');
  group.className = 'btn-group';
  if (yesno) {
    const y = document.createElement('button'); y.className = 'btn btn-accent'; y.textContent = 'Yes';
    const n = document.createElement('button'); n.className = 'btn btn-accent no'; n.textContent = 'No';
    group.appendChild(y); group.appendChild(n);
  }
  const f = document.createElement('button'); f.className = 'btn'; f.textContent = 'Focus';
  const x = document.createElement('button'); x.className = 'btn btn-icon'; x.textContent = '×';
  group.appendChild(f); group.appendChild(x);
  li.appendChild(group);

  return li;
}

function addNotif(state) {
  document.getElementById('pill').classList.remove('pill-hidden');
  document.getElementById('notif-list').appendChild(mkRow(state));
}

// --- Fake data for visual check ---
window.addEventListener('DOMContentLoaded', () => {
  addNotif({
    id: 'demo-1', event: 'Notification', basename: 'trading',
    message: 'Claude is waiting for your input [y/N]', yesno: true, source_type: 'vscode',
  });
  setTimeout(() => addNotif({
    id: 'demo-2', event: 'Stop', basename: 'nova',
    message: 'Done', yesno: false, source_type: 'wt',
  }), 1500);
});
