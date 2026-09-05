const stage = document.querySelector('#stage');
const panel = document.querySelector('#editor');
const notice = document.querySelector('#notice');
const returnButton = document.querySelector('#return');
let saved = JSON.parse(document.querySelector('#initial').textContent);
let draft = structuredClone(saved);
let backendRevision = 0;
let screen = null;
let inactive = true;
let previewScreen = 'music-select';
let selectedCanvas = draft[0]?.id ?? null;
let selectedWidget = null;
let editing = false;
let readonly = true;
let actualPreview = false;
let undo = null;
let drag = null;
let pendingDelete = null;
let placingKind = null;
let placingPoint = {x:0, y:0};
let socket;

const screenOptions = [
  ['SELECT', 'music-select'], ['MODE', 'mode-select'],
  ['DECIDE', 'decide-transition'], ['PLAY', 'play'], ['RESULT', 'result'],
];
const widgetOptions = [
  ['STATUS', 'status'], ['SELECTION', 'selection'], ['SCORE', 'score'],
  ['HISTORY LIST', 'history-list'], ['HISTORY GRAPH', 'history-graph'],
];
const sizes = {
  status: [560, 72], selection: [560, 120], score: [560, 300],
  'history-list': [560, 236], 'history-graph': [560, 280],
};
const snap = value => Math.round(value / 4) * 4;
const current = () => draft.find(canvas => canvas.id === selectedCanvas);
const visible = canvas => !canvas.show_on || canvas.show_on.includes(editing ? previewScreen : screen);
const nextId = (stem, values) => {
  for (let index = 1; ; index += 1) {
    const id = `${stem}-${index}`;
    if (!values.some(value => value.id === id)) return id;
  }
};
const send = value => socket?.readyState === WebSocket.OPEN && socket.send(JSON.stringify(value));
const draftChanged = () => {
  send({command:'update_backend_draft', backend:'obs', editor_id:editorId, canvases:draft});
  render();
};

function connect() {
  socket = new WebSocket(`ws://${location.host}/ws/stage`);
  socket.onmessage = event => {
    const message = JSON.parse(event.data);
    if (message.type === 'stage') {
      screen = message.state.screen.kind;
      inactive = message.state.system === 'inactive';
      if (!editing) {
        saved = message.canvases.map(layout => {
          const full = saved.find(canvas => canvas.id === layout.id);
          return full ? {...full, ...layout} : layout;
        });
        draft = structuredClone(saved);
      }
      render();
    }
    if (message.type === 'control') {
      const response = message.response;
      readonly = response.readonly;
      if (response.backend_revision !== null && response.backend_revision !== undefined) {
        backendRevision = response.backend_revision;
      }
      if (response.canvases?.length) {
        draft = structuredClone(response.canvases);
        if (!selectedCanvas || !draft.some(canvas => canvas.id === selectedCanvas)) {
          selectedCanvas = draft[0]?.id ?? null;
        }
      }
      if (response.error) showNotice(response.error, true);
      render();
    }
  };
  socket.onclose = () => setTimeout(connect, 1000);
}

function enterEditor(event) {
  event.preventDefault();
  if (editing) return;
  editing = true;
  actualPreview = false;
  readonly = true;
  send({command: 'acquire_backend', backend: 'obs', editor_id: editorId});
  render();
}

const editorId = `obs-${Date.now()}-${Math.random().toString(16).slice(2)}`;
document.body.addEventListener('contextmenu', enterEditor);
setInterval(() => {
  if (editing && !readonly) send({command: 'keep_alive_backend', backend: 'obs', editor_id: editorId});
}, 5000);

function showNotice(text, error = false) {
  notice.textContent = text;
  notice.className = error ? 'show error' : 'show';
  setTimeout(() => notice.className = '', 5000);
}

function save() {
  if (readonly) return;
  send({command: 'commit_backend', backend: 'obs', editor_id: editorId,
    expected_revision: backendRevision, canvases: draft});
  const wait = event => {
    const message = JSON.parse(event.data);
    if (message.type !== 'control') return;
    socket.removeEventListener('message', wait);
    if (!message.response.ok) return;
    saved = structuredClone(message.response.canvases);
    draft = structuredClone(saved);
    closeEditor();
  };
  socket.addEventListener('message', wait);
}

function discard() {
  draft = structuredClone(saved);
  closeEditor();
}

function closeEditor() {
  send({command: 'release_backend', backend: 'obs', editor_id: editorId});
  editing = false;
  actualPreview = false;
  selectedWidget = null;
  undo = null;
  render();
}

function renderStage() {
  const previewWidth = editing && !actualPreview ? innerWidth - 320 : innerWidth;
  const scale = editing && !actualPreview ? Math.min(1, previewWidth / innerWidth) : 1;
  stage.style.left = editing && !actualPreview ? '320px' : '0';
  stage.style.transform = `scale(${scale})`;
  stage.style.transformOrigin = 'top left';
  const seen = new Set();
  for (const canvas of draft) {
    if (!canvas.enabled || !visible(canvas)) continue;
    seen.add(canvas.id);
    let frame = stage.querySelector(`.stage-canvas[data-canvas="${CSS.escape(canvas.id)}"]`);
    if (!frame) {
      frame = document.createElement('div');
      frame.dataset.canvas = canvas.id;
      const iframe = document.createElement('iframe');
      iframe.tabIndex = -1;
      frame.append(iframe);
      stage.append(frame);
    }
    frame.className = `stage-canvas ${canvas.id === selectedCanvas && editing ? 'selected' : ''}`;
    frame.style.cssText = `left:${canvas.x}px;top:${canvas.y}px;width:${canvas.width}px;height:${canvas.height}px`;
    const iframe = frame.querySelector('iframe');
    const source = `/canvas/${encodeURIComponent(canvas.id)}${editing && inactive ? '?sample=1' : ''}`;
    if (iframe.getAttribute('src') !== source) iframe.src = source;
    frame.querySelectorAll('.stage-widget-hit,.canvas-resize').forEach(element => element.remove());
    if (editing && !actualPreview && canvas.id === selectedCanvas) {
      addWidgetHandles(frame, canvas, scale);
      for (const corner of ['nw', 'ne', 'sw', 'se']) {
        const handle = document.createElement('i');
        handle.className = `resize-handle canvas-resize ${corner}`;
        Object.assign(handle.style, {display:'block', position:'absolute', width:'10px', height:'10px',
          border:'1px solid white', background:'#101722', boxShadow:'0 0 7px #00ddff', zIndex:'20',
          transform:`scale(${1 / scale})`});
        handle.onpointerdown = event => startCanvasResize(event, canvas, corner, scale);
        frame.append(handle);
      }
    }
  }
  for (const frame of stage.querySelectorAll('.stage-canvas')) {
    if (!seen.has(frame.dataset.canvas)) frame.remove();
  }
  document.querySelector('.placement-ghost')?.remove();
  if (editing && placingKind && current()) {
    const ghost = document.createElement('div');
    const [width, height] = sizes[placingKind];
    ghost.className = 'placement-ghost';
    ghost.style.cssText = `position:fixed;left:${placingPoint.x}px;top:${placingPoint.y}px;width:${width * scale}px;height:${height * scale}px;border:1px dashed #5ee7ff;background:#0bd4ee22;pointer-events:none;z-index:2147483641`;
    document.body.append(ghost);
  }
  returnButton.style.display = actualPreview ? 'block' : 'none';
}
function addWidgetHandles(frame, canvas, scale) {
  for (const widget of canvas.widgets) {
    const hit = document.createElement('div');
    hit.dataset.widget = widget.id;
    hit.className = `stage-widget-hit ${widget.id === selectedWidget ? 'selected' : ''}`;
    hit.style.cssText = `left:${widget.x}px;top:${widget.y}px;width:${widget.width}px;height:${widget.height}px`;
    hit.title = widget.id;
    hit.onpointerdown = event => startWidgetDrag(event, canvas, widget, false, scale);
    for (const corner of ['nw', 'ne', 'sw', 'se']) {
      const handle = document.createElement('i');
      handle.className = `resize-handle ${corner}`;
      handle.onpointerdown = event => startWidgetDrag(event, canvas, widget, corner, scale);
      hit.append(handle);
    }
    frame.append(hit);
  }
}

function startWidgetDrag(event, canvas, widget, corner, scale) {
  if (readonly) return;
  event.preventDefault();
  event.stopPropagation();
  selectedWidget = widget.id;
  undo = {canvas: canvas.id, widget: widget.id, value: structuredClone(widget)};
  drag = {kind: 'widget', canvas, widget, corner, startX: event.clientX, startY: event.clientY,
    original: structuredClone(widget), scale};
  event.currentTarget.setPointerCapture(event.pointerId);
  renderPanel();
}

function startCanvasDrag(event, canvas) {
  if (readonly) return;
  event.preventDefault();
  selectedCanvas = canvas.id;
  selectedWidget = null;
  undo = {canvas: canvas.id, value: {x: canvas.x, y: canvas.y, width: canvas.width, height: canvas.height}};
  drag = {kind: 'canvas', canvas, startX: event.clientX, startY: event.clientY,
    original: {x: canvas.x, y: canvas.y}};
  event.currentTarget.setPointerCapture(event.pointerId);
}

function startCanvasResize(event, canvas, corner, scale) {
  if (readonly) return;
  event.preventDefault(); event.stopPropagation();
  undo = {canvas: canvas.id, value: {x:canvas.x, y:canvas.y, width:canvas.width, height:canvas.height}};
  drag = {kind:'canvas-resize', canvas, corner, scale, startX:event.clientX, startY:event.clientY,
    original:{x:canvas.x, y:canvas.y, width:canvas.width, height:canvas.height}};
  event.currentTarget.setPointerCapture(event.pointerId);
}

addEventListener('pointermove', event => {
  if (placingKind && !drag) {
    placingPoint = {x:event.clientX, y:event.clientY};
    const ghost = document.querySelector('.placement-ghost');
    if (ghost) { ghost.style.left = `${event.clientX}px`; ghost.style.top = `${event.clientY}px`; }
  }
  if (!drag) return;
  const dx = (event.clientX - drag.startX) / (drag.scale || 1);
  const dy = (event.clientY - drag.startY) / (drag.scale || 1);
  if (drag.kind === 'canvas') {
    drag.canvas.x = Math.max(0, Math.min(innerWidth - drag.canvas.width, snap(drag.original.x + dx)));
    drag.canvas.y = Math.max(0, Math.min(innerHeight - drag.canvas.height, snap(drag.original.y + dy)));
  } else if (drag.kind === 'canvas-resize') {
    const west = drag.corner.includes('w'); const north = drag.corner.includes('n');
    let left = west ? snap(drag.original.x + dx) : drag.original.x;
    let top = north ? snap(drag.original.y + dy) : drag.original.y;
    let right = west ? drag.original.x + drag.original.width : snap(drag.original.x + drag.original.width + dx);
    let bottom = north ? drag.original.y + drag.original.height : snap(drag.original.y + drag.original.height + dy);
    const minWidth = Math.max(32, ...drag.canvas.widgets.map(widget => widget.x + widget.width));
    const minHeight = Math.max(32, ...drag.canvas.widgets.map(widget => widget.y + widget.height));
    left = Math.max(0, Math.min(left, right - minWidth)); top = Math.max(0, Math.min(top, bottom - minHeight));
    right = Math.min(innerWidth, Math.max(right, left + minWidth)); bottom = Math.min(innerHeight, Math.max(bottom, top + minHeight));
    Object.assign(drag.canvas, {x:left, y:top, width:right-left, height:bottom-top});
  } else if (!drag.corner) {
    drag.widget.x = Math.max(0, Math.min(drag.canvas.width - drag.widget.width, snap(drag.original.x + dx)));
    drag.widget.y = Math.max(0, Math.min(drag.canvas.height - drag.widget.height, snap(drag.original.y + dy)));
  } else {
    const west = drag.corner.includes('w');
    const north = drag.corner.includes('n');
    let left = west ? snap(drag.original.x + dx) : drag.original.x;
    let top = north ? snap(drag.original.y + dy) : drag.original.y;
    let right = west ? drag.original.x + drag.original.width : snap(drag.original.x + drag.original.width + dx);
    let bottom = north ? drag.original.y + drag.original.height : snap(drag.original.y + drag.original.height + dy);
    left = Math.max(0, Math.min(left, right - 32));
    top = Math.max(0, Math.min(top, bottom - 32));
    right = Math.min(drag.canvas.width, Math.max(right, left + 32));
    bottom = Math.min(drag.canvas.height, Math.max(bottom, top + 32));
    Object.assign(drag.widget, {x:left, y:top, width:right-left, height:bottom-top});
  }
  updateStageGeometry();
});
addEventListener('pointerup', () => {
  const changed = drag !== null; drag = null;
  if (changed) draftChanged(); else render();
});
stage.addEventListener('click', event => {
  if (!placingKind || readonly) return;
  const canvas = current(); if (!canvas) return;
  const scale = Math.min(1, (innerWidth - 320) / innerWidth);
  const [naturalWidth, naturalHeight] = sizes[placingKind];
  const width = Math.min(naturalWidth, canvas.width); const height = Math.min(naturalHeight, canvas.height);
  const x = Math.max(0, Math.min(canvas.width - width, snap((event.clientX - 320) / scale - canvas.x)));
  const y = Math.max(0, Math.min(canvas.height - height, snap(event.clientY / scale - canvas.y)));
  const id = nextId(placingKind, canvas.widgets);
  canvas.widgets.push({id, kind:placingKind, x, y, width, height, settings:{history_count:5, graph_months:6}});
  selectedWidget = id; placingKind = null; draftChanged();
});

function updateStageGeometry() {
  for (const canvas of draft) {
    const frame = stage.querySelector(`[data-canvas="${CSS.escape(canvas.id)}"]`);
    if (!frame) continue;
    frame.style.left = `${canvas.x}px`; frame.style.top = `${canvas.y}px`;
    frame.style.width = `${canvas.width}px`; frame.style.height = `${canvas.height}px`;
    for (const widget of canvas.widgets) {
      const hit = frame.querySelector(`[data-widget="${CSS.escape(widget.id)}"]`);
      if (!hit) continue;
      hit.style.left = `${widget.x}px`; hit.style.top = `${widget.y}px`;
      hit.style.width = `${widget.width}px`; hit.style.height = `${widget.height}px`;
    }
  }
}

function undoGeometry() {
  if (!undo) return;
  const canvas = draft.find(item => item.id === undo.canvas);
  if (!canvas) return;
  if (undo.widget) {
    const index = canvas.widgets.findIndex(widget => widget.id === undo.widget);
    if (index >= 0) canvas.widgets[index] = undo.value;
  } else Object.assign(canvas, undo.value);
  undo = null;
  draftChanged();
}

function renderPanel() {
  panel.replaceChildren();
  if (!editing || actualPreview) { panel.style.display = 'none'; return; }
  panel.style.display = 'flex';
  const canvas = current();
  const previewScale = Math.min(1, (innerWidth - 320) / innerWidth);
  panel.innerHTML = `<header><strong>SCOREPEEK OVERLAY</strong><small>OBS EDITOR · PREVIEW ${Math.round(previewScale * 100)}%</small>${inactive ? '<b>SAMPLE DATA</b>' : ''}</header>`;
  const tabs = document.createElement('div'); tabs.className = 'preview-tabs';
  for (const [label, value] of screenOptions) {
    const button = document.createElement('button'); button.textContent = label;
    button.className = previewScreen === value ? 'active' : '';
    button.onclick = () => { previewScreen = value; render(); };
    tabs.append(button);
  }
  panel.append(tabs);
  const nav = document.createElement('nav');
  for (const item of draft) {
    const button = document.createElement('button');
    button.textContent = `${item.enabled ? '●' : '○'} ${item.id}`;
    button.className = item.id === selectedCanvas ? 'active' : '';
    button.onpointerdown = event => item.id === selectedCanvas ? startCanvasDrag(event, item) : null;
    button.onclick = () => { selectedCanvas = item.id; selectedWidget = null; pendingDelete = null; render(); };
    nav.append(button);
  }
  panel.append(nav);
  if (canvas) panel.append(canvasDetail(canvas));
  const footer = document.createElement('footer');
  for (const [label, action, primary] of [
    ['UNDO GEOMETRY', undoGeometry], ['PREVIEW ACTUAL', () => { actualPreview = true; render(); }],
    ['DISCARD', discard], ['SAVE AND CLOSE', save, true],
  ]) {
    const button = document.createElement('button'); button.textContent = label; button.onclick = action;
    button.disabled = readonly || (label === 'UNDO GEOMETRY' && !undo);
    if (primary) button.className = 'primary'; footer.append(button);
  }
  panel.append(footer);
}

function canvasDetail(canvas) {
  const section = document.createElement('section');
  const visibility = canvas.show_on ? canvas.show_on.map(value => screenOptions.find(item => item[1] === value)?.[0]).join(' / ') : 'ALWAYS';
  section.innerHTML = `<h2>${canvas.id}</h2><small>${visibility}</small>`;
  const widgets = document.createElement('div'); widgets.className = 'widget-list';
  for (const widget of canvas.widgets) {
    const row = document.createElement('button'); row.textContent = widget.id;
    row.className = widget.id === selectedWidget ? 'active' : '';
    row.onclick = () => { selectedWidget = widget.id; pendingDelete = null; renderPanel(); renderStage(); };
    widgets.append(row);
  }
  section.append(widgets);
  const selected = canvas.widgets.find(widget => widget.id === selectedWidget);
  if (selected) {
    const controls = document.createElement('details'); controls.open = true;
    controls.innerHTML = `<summary>${selected.id} SETTINGS</summary>`;
    const values = selected.kind === 'history-list' ? [5,10,20,50]
      : selected.kind === 'history-graph' ? [1,3,6,12] : [];
    const key = selected.kind === 'history-list' ? 'history_count' : 'graph_months';
    for (const value of values) {
      const button = document.createElement('button');
      button.textContent = selected.kind === 'history-graph' ? `${value}M` : `${value}`;
      button.className = selected.settings[key] === value ? 'active' : '';
      button.onclick = () => { selected.settings[key] = value; draftChanged(); };
      controls.append(button);
    }
    const deleteKey = `${canvas.id}/${selected.id}`;
    const remove = document.createElement('button');
    remove.textContent = pendingDelete === deleteKey ? 'CONFIRM DELETE' : 'DELETE WIDGET';
    remove.onclick = () => {
      if (pendingDelete !== deleteKey) { pendingDelete = deleteKey; renderPanel(); return; }
      canvas.widgets = canvas.widgets.filter(widget => widget.id !== selected.id);
      selectedWidget = null; pendingDelete = null; draftChanged();
    };
    controls.append(remove); section.append(controls);
  }
  const add = document.createElement('details');
  add.innerHTML = `<summary>＋ ADD WIDGET</summary>${placingKind ? '<small>配置位置をクリック</small>' : ''}`;
  for (const [label, kind] of widgetOptions) {
    const button = document.createElement('button'); button.textContent = label;
    button.onclick = () => {
      placingKind = kind; render();
    };
    add.append(button);
  }
  section.append(add);
  if (placingKind) {
    const cancel = document.createElement('button'); cancel.textContent = 'CANCEL PLACEMENT';
    cancel.onclick = () => { placingKind = null; render(); }; section.append(cancel);
  }
  section.append(settings(canvas));
  return section;
}

function settings(canvas) {
  const details = document.createElement('details');
  details.innerHTML = '<summary>CANVAS SETTINGS</summary><h3>VISIBILITY</h3>';
  const always = document.createElement('button'); always.textContent = 'ALWAYS';
  always.className = canvas.show_on ? '' : 'active'; always.onclick = () => { canvas.show_on = null; draftChanged(); };
  details.append(always);
  for (const [label, value] of screenOptions) {
    const button = document.createElement('button'); button.textContent = label;
    button.className = canvas.show_on?.includes(value) ? 'active' : '';
    button.onclick = () => {
      const values = canvas.show_on ? [...canvas.show_on] : [];
      const index = values.indexOf(value); index >= 0 ? values.splice(index, 1) : values.push(value);
      canvas.show_on = values.length ? values : null; draftChanged();
    };
    details.append(button);
  }
  const skins = document.createElement('div'); skins.innerHTML = '<h3>APPEARANCE</h3>';
  for (const [label, value] of [['CYAN','cyan-system'],['AURORA','result-aurora'],['BLACKBOX','dj-blackbox']]) {
    const button = document.createElement('button'); button.textContent = label;
    button.className = canvas.skin === value ? 'active' : ''; button.onclick = () => { canvas.skin = value; draftChanged(); };
    skins.append(button);
  }
  details.append(skins);
  const enabled = document.createElement('button'); enabled.textContent = canvas.enabled ? 'DISABLE CANVAS' : 'ENABLE CANVAS';
  enabled.onclick = () => { canvas.enabled = !canvas.enabled; draftChanged(); }; details.append(enabled);
  const remove = document.createElement('button'); remove.textContent = pendingDelete === canvas.id ? 'CONFIRM DELETE' : 'DELETE CANVAS';
  remove.disabled = draft.length <= 1; remove.onclick = () => {
    if (pendingDelete !== canvas.id) { pendingDelete = canvas.id; renderPanel(); return; }
    draft = draft.filter(item => item.id !== canvas.id); selectedCanvas = draft[0]?.id ?? null; pendingDelete = null; draftChanged();
  }; details.append(remove);
  const create = document.createElement('button'); create.textContent = 'ADD EMPTY CANVAS'; create.onclick = () => {
    const id = nextId('obs-canvas', draft);
    draft.push({id, enabled:true, skin:'cyan-system', revision:0, show_on:null, opacity_percent:100, output:null, x:0, y:0, width:snap(Math.min(560, innerWidth)), height:snap(Math.min(1040, innerHeight)), widgets:[]});
    selectedCanvas = id; draftChanged();
  }; details.append(create);
  return details;
}

function render() { renderStage(); renderPanel(); }
returnButton.onclick = () => { actualPreview = false; render(); };
addEventListener('resize', render);
connect();
render();
