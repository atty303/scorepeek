const stage = document.querySelector('#stage');
const panel = document.querySelector('#editor');
const notice = document.querySelector('#notice');
const returnButton = document.querySelector('#return');
const panelToggle = document.querySelector('#panel-toggle');
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
let panelOpen = true;
let editorTab = 'widgets';
let widgetAddOpen = false;
let manageOpen = false;
let undo = null;
let drag = null;
let pendingDelete = null;
let placingKind = null;
let placingPoint = {x:0, y:0};
let nextRequestId = 1;
let presentationGeneration = 0;
let discardPending = false;
const pendingControls = new Map();
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
const gridFloor = value => Math.max(0, Math.floor(value / 4) * 4);
const current = () => draft.find(canvas => canvas.id === selectedCanvas);
const isDirty = () => JSON.stringify(draft) !== JSON.stringify(saved);
const visible = canvas => !canvas.show_on || canvas.show_on.includes(editing ? previewScreen : screen);
const nextId = (stem, values) => {
  for (let index = 1; ; index += 1) {
    const id = `${stem}-${index}`;
    if (!values.some(value => value.id === id)) return id;
  }
};
const send = (request, callback) => {
  if (socket?.readyState !== WebSocket.OPEN) return false;
  const requestId = nextRequestId++;
  if (callback) pendingControls.set(requestId, callback);
  socket.send(JSON.stringify({request_id: requestId, request}));
  return true;
};
const draftChanged = () => {
  send({command:'update_backend_draft', backend:'obs', editor_id:editorId, canvases:draft});
  render();
};

function connect() {
  socket = new WebSocket(`ws://${location.host}/ws/stage`);
  socket.onopen = () => {
    if (editing) send({command: 'acquire_backend', backend: 'obs', editor_id: editorId}, response => {
      if (response.ok && !response.readonly && discardPending) sendDiscardRelease();
    });
  };
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
        if (!response.dirty) saved = structuredClone(response.canvases);
        presentationGeneration += 1;
        if (!selectedCanvas || !draft.some(canvas => canvas.id === selectedCanvas)) {
          selectedCanvas = draft[0]?.id ?? null;
        }
      }
      if (response.error) showNotice(response.error, true);
      const callback = pendingControls.get(message.request_id);
      if (callback) {
        pendingControls.delete(message.request_id);
        callback(response);
      }
      render();
    }
  };
  socket.onclose = () => {
    pendingControls.clear();
    readonly = true;
    render();
    setTimeout(connect, 1000);
  };
}

function enterEditor(event) {
  event.preventDefault();
  if (editing) return;
  editing = true;
  panelOpen = true;
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
    expected_revision: backendRevision, canvases: draft}, response => {
    if (!response.ok) return;
    saved = structuredClone(response.canvases);
    draft = structuredClone(saved);
    closeEditor();
  });
}

function discard() {
  if (readonly || discardPending) return;
  discardPending = true;
  sendDiscardRelease();
}

function sendDiscardRelease() {
  if (!send({command: 'release_backend', backend: 'obs', editor_id: editorId}, response => {
    if (!response.ok) {
      discardPending = false;
      render();
      return;
    }
    discardPending = false;
    finishClose();
  })) {
    readonly = true;
    showNotice('Editor connection lost; reconnecting before discard.', true);
    render();
  }
}

function closeEditor() {
  send({command: 'release_backend', backend: 'obs', editor_id: editorId});
  finishClose();
}

function finishClose() {
  editing = false;
  discardPending = false;
  actualPreview = false;
  selectedWidget = null;
  undo = null;
  render();
}

function renderStage() {
  const scale = 1;
  stage.style.left = '0';
  stage.style.transform = 'none';
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
    const query = new URLSearchParams();
    if (editing && inactive) query.set('sample', '1');
    query.set('presentation', `${presentationGeneration}`);
    const source = `/canvas/${encodeURIComponent(canvas.id)}?${query}`;
    if (iframe.getAttribute('src') !== source) iframe.src = source;
    frame.onpointerdown = event => {
      if (editing && !actualPreview && event.button === 2) startCanvasDrag(event, canvas);
    };
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
  panelToggle.style.display = editing && !actualPreview ? 'block' : 'none';
  panelToggle.className = isDirty() ? 'dirty' : '';
  panelToggle.innerHTML = `${panelOpen ? '‹' : '›'}<i></i>`;
  panelToggle.setAttribute('aria-label', panelOpen ? 'Hide editor panel' : 'Show editor panel');
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
  if (readonly || event.button !== 0) return;
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
  if (readonly || event.button !== 2) return;
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
    drag.canvas.x = Math.max(0, Math.min(gridFloor(innerWidth - drag.canvas.width), snap(drag.original.x + dx)));
    drag.canvas.y = Math.max(0, Math.min(gridFloor(innerHeight - drag.canvas.height), snap(drag.original.y + dy)));
  } else if (drag.kind === 'canvas-resize') {
    const west = drag.corner.includes('w'); const north = drag.corner.includes('n');
    let left = west ? snap(drag.original.x + dx) : drag.original.x;
    let top = north ? snap(drag.original.y + dy) : drag.original.y;
    let right = west ? drag.original.x + drag.original.width : snap(drag.original.x + drag.original.width + dx);
    let bottom = north ? drag.original.y + drag.original.height : snap(drag.original.y + drag.original.height + dy);
    const minWidth = Math.max(32, ...drag.canvas.widgets.map(widget => widget.x + widget.width));
    const minHeight = Math.max(32, ...drag.canvas.widgets.map(widget => widget.y + widget.height));
    left = Math.max(0, Math.min(left, right - minWidth)); top = Math.max(0, Math.min(top, bottom - minHeight));
    right = Math.min(gridFloor(innerWidth), Math.max(right, left + minWidth)); bottom = Math.min(gridFloor(innerHeight), Math.max(bottom, top + minHeight));
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
  const scale = 1;
  const [naturalWidth, naturalHeight] = sizes[placingKind];
  const width = Math.min(naturalWidth, canvas.width); const height = Math.min(naturalHeight, canvas.height);
  const x = Math.max(0, Math.min(canvas.width - width, snap(event.clientX / scale - canvas.x)));
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
  if (!editing || actualPreview || !panelOpen) { panel.style.display = 'none'; return; }
  panel.style.display = 'flex';
  panel.style.width = `${Math.max(360, Math.min(480, Math.round(innerWidth / 5)))}px`;
  const canvas = current();
  panel.innerHTML = `<header><strong>SCOREPEEK OVERLAY</strong><small>OBS EDITOR</small>${inactive ? '<b>SAMPLE DATA</b>' : ''}${isDirty() ? '<i class="unsaved-dot"></i>' : ''}</header><p class="section-label">PREVIEW DATA</p>`;
  const tabs = document.createElement('div'); tabs.className = 'preview-tabs';
  for (const [label, value] of screenOptions) {
    const button = document.createElement('button'); button.textContent = label;
    button.className = previewScreen === value ? 'selected' : '';
    button.setAttribute('aria-selected', previewScreen === value);
    button.onclick = () => { previewScreen = value; render(); };
    tabs.append(button);
  }
  panel.append(tabs);
  const nav = document.createElement('nav');
  for (const item of draft) {
    const button = document.createElement('button');
    button.className = `canvas-row ${item.id === selectedCanvas ? 'selected' : ''} ${item.enabled ? '' : 'disabled'}`;
    button.setAttribute('aria-selected', item.id === selectedCanvas);
    button.innerHTML = `<span>${item.id}</span><i class="canvas-enabled-lamp ${item.enabled ? 'on' : 'off'}"></i>`;
    button.onclick = () => { selectedCanvas = item.id; selectedWidget = null; pendingDelete = null; render(); };
    nav.append(button);
  }
  panel.append(nav);
  const workTabs = document.createElement('div'); workTabs.className = 'editor-tabs';
  for (const [label, value] of [['WIDGETS','widgets'],['CANVAS','canvas']]) {
    const button = document.createElement('button'); button.textContent = label;
    button.className = editorTab === value ? 'selected' : '';
    button.setAttribute('aria-selected', editorTab === value);
    button.onclick = () => { editorTab = value; renderPanel(); };
    workTabs.append(button);
  }
  panel.append(workTabs);
  const body = document.createElement('div'); body.className = 'editor-tab-body';
  if (canvas) body.append(editorTab === 'widgets' ? widgetDetail(canvas) : canvasSettings(canvas));
  panel.append(body);
  const footer = document.createElement('footer');
  for (const [label, action, primary] of [
    ['UNDO GEOMETRY', undoGeometry], ['PREVIEW ACTUAL', () => { actualPreview = true; render(); }],
    ['DISCARD', discard], ['SAVE AND CLOSE', save, true],
  ]) {
    const button = document.createElement('button'); button.textContent = label; button.onclick = action;
    button.disabled = readonly || discardPending || (label === 'UNDO GEOMETRY' && !undo) || ((label === 'DISCARD' || primary) && !isDirty());
    if (primary && isDirty()) button.className = 'primary'; footer.append(button);
  }
  panel.append(footer);
}

function widgetDetail(canvas) {
  const section = document.createElement('section');
  const widgets = document.createElement('div'); widgets.className = 'widget-list';
  for (const widget of canvas.widgets) {
    const row = document.createElement('button'); row.textContent = widget.id;
    row.className = widget.id === selectedWidget ? 'selected' : '';
    row.setAttribute('aria-selected', widget.id === selectedWidget);
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
      button.className = selected.settings[key] === value ? 'selected' : '';
      button.setAttribute('aria-pressed', selected.settings[key] === value);
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
  add.open = widgetAddOpen;
  add.ontoggle = () => { widgetAddOpen = add.open; };
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
  return section;
}

function canvasSettings(canvas) {
  const section = document.createElement('section');
  section.innerHTML = '<h2>VISIBILITY</h2>';
  const modes = document.createElement('div'); modes.className = 'visibility-mode';
  for (const [label, specific] of [['ALL SCREENS', false], ['SPECIFIC SCREENS', true]]) {
    const selected = specific ? !!canvas.show_on : !canvas.show_on;
    const button = document.createElement('button'); button.textContent = label;
    button.className = selected ? 'selected' : ''; button.setAttribute('aria-pressed', selected);
    button.onclick = () => { canvas.show_on = specific ? [previewScreen] : null; draftChanged(); };
    modes.append(button);
  }
  section.append(modes);
  if (canvas.show_on) {
    const screens = document.createElement('div'); screens.className = 'screen-filter';
    for (const [label, value] of screenOptions) {
      const selected = canvas.show_on.includes(value);
      const button = document.createElement('button'); button.textContent = `${selected ? '✓ ' : ''}${label}`;
      button.className = selected ? 'selected' : ''; button.setAttribute('aria-pressed', selected);
      button.onclick = () => {
        const values = [...canvas.show_on]; const index = values.indexOf(value);
        index >= 0 ? values.splice(index, 1) : values.push(value);
        canvas.show_on = values.length ? values : null; draftChanged();
      };
      screens.append(button);
    }
    section.append(screens);
  }
  const skins = document.createElement('div'); skins.innerHTML = '<h2>APPEARANCE</h2>'; skins.className = 'skin-settings';
  for (const [label, value] of [['CYAN','cyan-system'],['AURORA','result-aurora'],['BLACKBOX','dj-blackbox']]) {
    const button = document.createElement('button'); button.textContent = label;
    button.className = canvas.skin === value ? 'selected' : ''; button.setAttribute('aria-pressed', canvas.skin === value);
    button.onclick = () => { canvas.skin = value; draftChanged(); };
    skins.append(button);
  }
  section.append(skins);
  const manage = document.createElement('details'); manage.innerHTML = '<summary>MANAGE CANVAS</summary>';
  manage.open = manageOpen;
  manage.ontoggle = () => { manageOpen = manage.open; };
  const enabled = document.createElement('button'); enabled.innerHTML = `<span>CANVAS ENABLED</span><i>${canvas.enabled ? '✓' : ''}</i>`;
  enabled.className = `canvas-switch ${canvas.enabled ? 'selected' : ''}`; enabled.setAttribute('aria-pressed', canvas.enabled);
  enabled.onclick = () => { canvas.enabled = !canvas.enabled; draftChanged(); }; manage.append(enabled);
  const remove = document.createElement('button'); remove.textContent = pendingDelete === canvas.id ? 'CONFIRM DELETE' : 'DELETE CANVAS';
  remove.className = 'danger';
  remove.disabled = draft.length <= 1; remove.onclick = () => {
    if (pendingDelete !== canvas.id) { pendingDelete = canvas.id; renderPanel(); return; }
    draft = draft.filter(item => item.id !== canvas.id); selectedCanvas = draft[0]?.id ?? null; pendingDelete = null; draftChanged();
  }; manage.append(remove);
  const create = document.createElement('button'); create.textContent = 'ADD EMPTY CANVAS'; create.onclick = () => {
    const id = nextId('obs-canvas', draft);
    draft.push({id, enabled:true, skin:'cyan-system', revision:0, show_on:null, opacity_percent:100, output:null, x:0, y:0, width:gridFloor(Math.min(560, innerWidth)), height:gridFloor(Math.min(1040, innerHeight)), widgets:[]});
    selectedCanvas = id; draftChanged();
  }; manage.append(create);
  section.append(manage);
  return section;
}

function render() { renderStage(); renderPanel(); }
returnButton.onclick = () => { actualPreview = false; render(); };
panelToggle.onclick = () => { panelOpen = !panelOpen; render(); };
addEventListener('resize', render);
connect();
render();
