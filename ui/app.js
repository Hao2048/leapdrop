// 跃传 LeapDrop 前端：设备雷达 / 拖拽发送 / 接收确认 / 传输进度
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (s) => document.querySelector(s);
const state = {
  peers: [],
  trusted: [],
  target: null,      // 选中的目标设备 id
  me: null,
  transfers: [],     // {id, dir, peer, name, pct, bytesDone, totalBytes, speed, status}
  incoming: null,    // {token, peer, files, totalBytes}
};

/* ── 工具 ── */
function fmtBytes(n) {
  if (n == null) return '—';
  if (n < 1024) return `${n} B`;
  if (n < 1024 ** 2) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 ** 3) return `${(n / 1024 ** 2).toFixed(1)} MB`;
  return `${(n / 1024 ** 3).toFixed(2)} GB`;
}
function fmtSpeed(bps) {
  if (!bps || bps < 1024) return `${Math.round(bps || 0)} B/s`;
  if (bps < 1024 ** 2) return `${(bps / 1024).toFixed(0)} KB/s`;
  return `${(bps / 1024 ** 2).toFixed(1)} MB/s`;
}
function ext(name) {
  const m = name.match(/\.([A-Za-z0-9]+)$/);
  return m ? m[1].slice(0, 4).toUpperCase() : 'FILE';
}
const EXT_COLORS = { PDF: '#d9453f', PPT: '#d97528', ZIP: '#c9a227', MP4: '#4a78d0', PNG: '#2e9e5b', JPG: '#2e9e5b', MOV: '#7057c9', KEY: '#3a6ea5' };
function extColor(name) { return EXT_COLORS[ext(name)] || '#6b7f8b'; }

function toast(msg, kind = '') {
  const el = document.createElement('div');
  el.className = `toast ${kind}`;
  el.textContent = msg;
  $('#toasts').appendChild(el);
  setTimeout(() => el.remove(), 3600);
}

function hashStr(s) {
  let h = 0;
  for (const c of s) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return h;
}

/* ── 视图切换 ── */
document.querySelectorAll('.rail button').forEach((b) =>
  b.addEventListener('click', () => {
    document.querySelectorAll('.rail button').forEach((x) => x.classList.toggle('on', x === b));
    document.querySelectorAll('.pane').forEach((p) => p.classList.toggle('is-active', p.id === `view-${b.dataset.view}`));
  })
);
function gotoView(name) {
  document.querySelector(`.rail button[data-view="${name}"]`)?.click();
}

/* ── 设备雷达 ── */
function renderPeers() {
  const layer = $('#devLayer');
  layer.innerHTML = '';
  $('#radarEmpty').style.display = state.peers.length ? 'none' : '';
  $('#netpill').className = 'netpill' + (state.peers.length ? '' : ' off');
  $('#netpill').innerHTML = `<i></i>${state.peers.length ? `已发现 ${state.peers.length} 台设备` : '扫描中…'}`;

  // 若选中目标已离线则清空
  if (state.target && !state.peers.some((p) => p.id === state.target)) {
    state.target = null;
    renderTarget();
  }

  state.peers.forEach((p, i) => {
    const angle = (hashStr(p.id) % 360) * Math.PI / 180;
    const radius = 28 + (hashStr(p.id) % 3) * 9; // 28% / 37% / 46%
    const x = 50 + radius * Math.cos(angle);
    const y = 50 + radius * Math.sin(angle);
    const el = document.createElement('button');
    el.className = 'dev' + (state.target === p.id ? ' selected' : '');
    el.style.left = `${x}%`;
    el.style.top = `${y}%`;
    el.innerHTML = `<span class="tick">✓ 已选中</span><span class="pairflag ${p.paired ? '' : 'no'}">${p.paired ? '已配对' : '未配对'}</span><b title="${p.name}">${p.name}</b><small class="num">${p.device_id || '????-????'} · ${p.os || ''}</small>`;
    el.addEventListener('click', () => {
      state.target = state.target === p.id ? null : p.id;
      renderPeers();
    });
    layer.appendChild(el);
  });
}

function renderTarget() {
  const p = state.peers.find((x) => x.id === state.target);
  $('#dzTarget').textContent = p ? `发送到「${p.name}」` : '先点选上方设备';
}

/* ── 发送 ── */
async function doSend(paths) {
  if (!state.target) {
    toast('请先在雷达中点选一台目标设备');
    return;
  }
  if (!paths?.length) return;
  try {
    await invoke('send_files', { target: state.target, paths });
  } catch (e) {
    toast(`发送失败：${e}`, 'err');
  }
}

$('#dropzone').addEventListener('click', async () => {
  const files = await invoke('pick_files');
  doSend(files);
});

// 拖拽（Tauri webview 事件）
window.__TAURI__.webview.getCurrentWebview().onDragDropEvent((e) => {
  const ev = e.payload;
  if (ev.type === 'over' || ev.type === 'enter') {
    $('#dropzone').classList.add('drag');
  } else if (ev.type === 'leave') {
    $('#dropzone').classList.remove('drag');
  } else if (ev.type === 'drop') {
    $('#dropzone').classList.remove('drag');
    doSend(ev.paths);
  }
});

/* ── 传输列表 ── */
function upsertTransfer(t) {
  const i = state.transfers.findIndex((x) => x.id === t.id);
  if (i >= 0) state.transfers[i] = { ...state.transfers[i], ...t };
  else state.transfers.unshift(t);
  state.transfers = state.transfers.slice(0, 50);
  renderTransfers();
}

function renderTransfers() {
  const list = $('#transferList');
  if (!state.transfers.length) {
    list.innerHTML = '<div class="empty-block">还没有传输记录<small>在「设备」页选一台设备，拖入文件即可发送</small></div>';
    return;
  }
  list.innerHTML = state.transfers
    .map((t) => {
      const pct = t.totalBytes ? Math.round((t.bytesDone / t.totalBytes) * 100) : t.pct || 0;
      const badge =
        t.status === 'done' ? '<span class="badge ok">完成</span>' :
        t.status === 'err' ? '<span class="badge err">失败</span>' :
        t.status === 'rejected' ? '<span class="badge err">已拒绝</span>' :
        t.status === 'pairing' ? '<span class="badge wait">等待对方确认配对</span>' :
        t.status === 'wait' ? '<span class="badge wait">等待对方确认</span>' :
        '<span class="badge run">传输中</span>';
      return `<div class="trow ${t.status === 'done' ? 'done' : ''} ${t.status === 'err' || t.status === 'rejected' ? 'err' : ''}">
        <div class="top">
          <span class="dir ${t.dir}">${t.dir === 'tx' ? '发送' : '接收'}</span>
          <span class="fname" title="${t.name}">${t.name}</span>
          ${badge}
        </div>
        <p class="peer">${t.dir === 'tx' ? '发至' : '来自'} ${t.peer}</p>
        <div class="bar"><i style="width:${t.status === 'done' ? 100 : pct}%"></i></div>
        <div class="stats">
          <span class="num">${fmtBytes(t.bytesDone)} / ${fmtBytes(t.totalBytes)}</span>
          <span class="num">${fmtSpeed(t.speed)}</span>
          ${t.status === 'run' ? `<span class="num">${pct}%</span>` : ''}
        </div>
      </div>`;
    })
    .join('');
}

$('#clearTransfers').addEventListener('click', () => {
  state.transfers = [];
  renderTransfers();
});

/* ── 接收确认弹窗 ── */
function showIncoming(payload) {
  state.incoming = payload;
  $('#inTitle').textContent = `「${payload.peer}」向你发送文件`;
  $('#inFrom').innerHTML = `来源 ${payload.peer} · 局域网直连`;
  $('#inFiles').innerHTML = payload.files
    .map(
      (f) => `<div class="frow">
        <span class="fico" style="background:${extColor(f.name)}">${ext(f.name)}</span>
        <span class="fname" title="${f.name}">${f.name}</span>
        <span class="fsize num">${fmtBytes(f.size)}</span>
      </div>`
    )
    .join('');
  $('#inCount').textContent = `共 ${payload.files.length} 项`;
  $('#inSize').textContent = fmtBytes(payload.totalBytes);
  $('#veil').hidden = false;
}
function closeIncoming() {
  $('#veil').hidden = true;
  state.incoming = null;
}
$('#inAccept').addEventListener('click', async () => {
  if (!state.incoming) return;
  const inc = state.incoming;
  await invoke('respond_incoming', { token: inc.token, accept: true });
  closeIncoming();
  upsertTransfer({
    id: 'rx-' + inc.token,
    dir: 'rx',
    peer: inc.peer,
    name: inc.files.length > 1 ? `${inc.files.length} 个文件` : inc.files[0].name,
    bytesDone: 0,
    totalBytes: inc.totalBytes,
    speed: 0,
    status: 'run',
  });
  gotoView('transfers');
});
$('#inReject').addEventListener('click', async () => {
  if (!state.incoming) return;
  await invoke('respond_incoming', { token: state.incoming.token, accept: false });
  closeIncoming();
});

/* ── 事件订阅 ── */
listen('peers', (e) => {
  state.peers = e.payload;
  renderPeers();
  renderTarget();
});
listen('incoming', (e) => showIncoming(e.payload));
listen('incoming-closed', () => closeIncoming());

/* ── 配对 ── */
let pairToken = null;
listen('pair-incoming', (e) => {
  pairToken = e.payload.token;
  $('#pairName').textContent = e.payload.name;
  $('#pairId').textContent = e.payload.deviceId;
  $('#pairVeil').hidden = false;
});
listen('pair-closed', () => { $('#pairVeil').hidden = true; pairToken = null; });
$('#pairAccept').addEventListener('click', async () => {
  if (!pairToken) return;
  await invoke('respond_pair', { token: pairToken, accept: true });
  $('#pairVeil').hidden = true;
  pairToken = null;
});
$('#pairReject').addEventListener('click', async () => {
  if (!pairToken) return;
  await invoke('respond_pair', { token: pairToken, accept: false });
  $('#pairVeil').hidden = true;
  pairToken = null;
});
listen('paired', (e) => toast(`已与「${e.payload.name}」完成配对`, 'ok'));
listen('tx-pairing', (e) => {
  upsertTransfer({
    id: txByPeer[e.payload.peer] || ('tx-' + ++txSeq),
    dir: 'tx', peer: e.payload.peer,
    name: `首次传输需先配对（${e.payload.peerName} ${e.payload.peerId}）`,
    bytesDone: 0, totalBytes: null, speed: 0, status: 'pairing',
  });
  txByPeer[e.payload.peer] = state.transfers[0]?.id;
  gotoView('transfers');
});
listen('trusted-changed', (e) => {
  state.trusted = e.payload || [];
  renderTrusted();
});

function renderTrusted() {
  const box = $('#trustedList');
  if (!state.trusted || !state.trusted.length) {
    box.innerHTML = '<div class="srow"><div class="t"><small>还没有配对过的设备。首次向一台设备发送文件时，双方确认一次即完成配对。</small></div></div>';
    return;
  }
  box.innerHTML = state.trusted
    .map((t) => `<div class="trow-row">
      <div class="tname">${t.name}<div class="tid num">${t.id}</div></div>
      <button class="remove" data-id="${t.id}">解除配对</button>
    </div>`)
    .join('');
  box.querySelectorAll('.remove').forEach((b) =>
    b.addEventListener('click', async () => {
      await invoke('remove_trusted', { deviceId: b.dataset.id });
      toast('已解除配对');
    })
  );
}

const txByPeer = {}; // 对端名 → 传输条目 id（一次会话一行）
let txSeq = 0;
listen('tx-start', (e) => {
  const id = 'tx-' + ++txSeq;
  txByPeer[e.payload.peer] = id;
  upsertTransfer({
    id,
    dir: 'tx',
    peer: e.payload.peer,
    name: `${e.payload.files} 个文件`,
    bytesDone: 0,
    totalBytes: null,
    speed: 0,
    status: 'wait',
  });
  gotoView('transfers');
});
listen('tx-progress', (e) => {
  const p = e.payload;
  const id = txByPeer[p.peer] || (txByPeer[p.peer] = 'tx-' + ++txSeq);
  upsertTransfer({
    id,
    dir: 'tx',
    peer: p.peer,
    name: p.fileName,
    bytesDone: p.bytesDone,
    totalBytes: p.totalBytes,
    speed: p.speedBps,
    status: 'run',
  });
});
listen('tx-done', (e) => {
  const id = txByPeer[e.payload.peer];
  if (id) upsertTransfer({ id, status: 'done', speed: 0, bytesDone: e.payload.bytes });
  delete txByPeer[e.payload.peer];
  toast(`已发送到「${e.payload.peer}」`, 'ok');
});
listen('tx-rejected', (e) => {
  const id = txByPeer[e.payload.peer];
  if (id) upsertTransfer({ id, status: 'rejected' });
  delete txByPeer[e.payload.peer];
  toast(e.payload.message || '对方拒绝了传输', 'err');
});
listen('tx-error', (e) => {
  const id = txByPeer[e.payload.peer];
  if (id) upsertTransfer({ id, status: 'err' });
  delete txByPeer[e.payload.peer];
  toast(`发送失败：${e.payload.message}`, 'err');
});
listen('rx-progress', (e) => {
  const p = e.payload;
  let t = state.transfers.find((x) => x.dir === 'rx' && x.status === 'run');
  if (!t) {
    // 用户在其他端点了接受（弹窗已被关闭的兜底）
    upsertTransfer({
      id: 'rx-' + p.token + '-' + ++txSeq,
      dir: 'rx',
      peer: '对端设备',
      name: p.fileName,
      bytesDone: 0,
      totalBytes: null,
      speed: 0,
      status: 'run',
    });
    t = state.transfers[0];
  }
  t.bytesDone = (t.bytesDone || 0) + p.bytes;
  t.speed = (t.speed || 0) * 0.5 + p.bytes * 4 * 0.5; // 平滑（4MB/块）
  t.name = p.fileName;
  upsertTransfer(t);
});
listen('rx-done', (e) => {
  const t = state.transfers.find((x) => x.dir === 'rx' && x.status === 'run');
  if (t) {
    t.status = e.payload.ok ? 'done' : 'err';
    if (e.payload.ok) t.bytesDone = t.totalBytes;
    upsertTransfer(t);
  }
  if (e.payload.ok) toast(`已保存 ${e.payload.saved.length} 个文件到下载目录`, 'ok');
  else toast('接收校验失败，文件已保留待续传', 'err');
});
listen('name-changed', (e) => {
  if (state.me) state.me.name = e.payload.name;
  $('#myName').textContent = e.payload.name;
  toast('设备名已更新');
});

/* ── 初始化 ── */
(async () => {
  const s = await invoke('get_state');
  state.me = s;
  $('#myName').textContent = s.name;
  $('#myOs').textContent = `本机 · ${s.os}`;
  $('#myAvatar').textContent = (s.name || '?').slice(0, 1);
  $('#fpText').textContent = s.fp;
  $('#myDeviceId').textContent = s.device_id;
  $('#deviceIdText').textContent = s.device_id;
  state.trusted = s.trusted || [];
  renderTrusted();
  $('#dlDir').textContent = s.download_dir;
  $('#nameInput').value = s.name;
  renderTransfers();
})();

$('#saveName').addEventListener('click', async () => {
  const v = $('#nameInput').value.trim();
  try {
    await invoke('set_device_name', { name: v });
  } catch (e) {
    toast(`改名失败：${e}`, 'err');
  }
});

/* ── 首启权限引导（点「知道了」后不再显示） ── */
(function () {
  const banner = document.getElementById('permBanner');
  if (!banner) return;
  if (!localStorage.getItem('leapdrop-perm-hint-dismissed')) banner.hidden = false;
  document.getElementById('permClose').addEventListener('click', () => {
    banner.hidden = true;
    localStorage.setItem('leapdrop-perm-hint-dismissed', '1');
  });
})();
