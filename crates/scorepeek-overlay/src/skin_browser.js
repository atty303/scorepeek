(() => {
  let spec = JSON.parse(document.querySelector("#scorepeek-skin").textContent);
  const wasmUrl = new URL(spec.wasm, location.href).href;
  const root = document.querySelector("#skin-root");
  let worker;
  let timer;
  let call = 0;
  let ready = false;
  let busy = false;
  let pending = false;
  let started = 0;
  let phase = "init";
  let state = { screen: "unknown" };
  let lastEditorSession;
  let lastEditorRevision = -1;
  let awaitingEditorGeometry = false;

  const workerSource = `
    let instance;
    function invoke(name, input) {
      const bytes = new TextEncoder().encode(JSON.stringify(input));
      const pointer = instance.exports.scorepeek_alloc(bytes.length);
      new Uint8Array(instance.exports.memory.buffer, pointer, bytes.length).set(bytes);
      const packed = instance.exports[name](pointer, bytes.length);
      const outputPointer = Number(BigInt.asUintN(64, packed) >> 32n);
      const outputLength = Number(BigInt.asUintN(64, packed) & 0xffffffffn);
      const output = new Uint8Array(instance.exports.memory.buffer, outputPointer, outputLength);
      const decoded = JSON.parse(new TextDecoder().decode(output));
      instance.exports.scorepeek_dealloc(pointer, bytes.length);
      return decoded;
    }
    onmessage = async event => {
      let stage = "render";
      try {
        if (event.data.type === "start") {
          stage = "compile";
          const module = await WebAssembly.compileStreaming(fetch(event.data.url));
          stage = "instantiate";
          instance = await WebAssembly.instantiate(module, {});
          stage = "init";
          postMessage({ id:event.data.id, output:invoke("scorepeek_init", event.data.input) });
        } else {
          postMessage({ id:event.data.id, output:invoke("scorepeek_render", event.data.input) });
        }
      } catch (_) { postMessage({ id:event.data.id, error:stage }); }
    }`;

  function input() {
    return { schema:"scorepeek-skin-input-v1", backend:"obs", canvas:spec.canvas,
      widgets:spec.widgets, state };
  }
  function start() {
    worker = new Worker(URL.createObjectURL(new Blob([workerSource], {type:"text/javascript"})));
    worker.onmessage = receive;
    ready = false;
    busy = false;
    pending = false;
    send("start", input());
  }
  function send(type, value) {
    if (!worker || busy || (type === "render" && !ready)) {
      if (type === "render") pending = true;
      return;
    }
    const id = ++call;
    busy = true;
    started = performance.now();
    phase = type === "start" ? "init" : "render";
    worker.postMessage({type, id, input:value, url:wasmUrl});
    clearTimeout(timer);
    timer = setTimeout(() => fail("hard_timeout"), 2000);
  }
  function receive(event) {
    if (event.data.id !== call) return;
    clearTimeout(timer);
    busy = false;
    if (event.data.error) return fail(event.data.error);
    ready = true;
    try {
      validateOutput(event.data.output);
      apply(event.data.output.tree);
      if (socket.readyState === WebSocket.OPEN) socket.send(JSON.stringify({type:"skin_diagnostic", status:"success", phase, duration_us:Math.round((performance.now()-started)*1000), next_tick:event.data.output.schedule.kind}));
      schedule(event.data.output.schedule);
      if (pending) { pending = false; queueMicrotask(requestRender); }
    }
    catch (error) { fail(["output","schedule","tree"].includes(error.message) ? "invalid_output" : "tree_apply_failed"); }
  }
  function requestRender() { send("render", input()); }
  function fail(reason) {
    clearTimeout(timer);
    if (worker) worker.terminate();
    worker = undefined;
    ready = false;
    busy = false;
    pending = false;
    root.replaceChildren();
    document.documentElement.dataset.skinFailure = reason;
    if (socket.readyState === WebSocket.OPEN) socket.send(JSON.stringify({type:"skin_diagnostic", status:"failed", error_type:reason}));
  }
  function schedule(value) {
    if (!worker || document.hidden || value.kind === "idle") return;
    if (value.kind === "next-frame") requestAnimationFrame(requestRender);
    if (value.kind === "after-ms") timer = setTimeout(requestRender, value.milliseconds);
  }
  function validateOutput(output) {
    if (!output || typeof output !== "object" || !output.schedule || !output.tree) throw new Error("output");
    const schedule = output.schedule;
    if (schedule.kind !== "idle" && schedule.kind !== "next-frame" && schedule.kind !== "after-ms") throw new Error("schedule");
    if (schedule.kind === "after-ms" && (!Number.isSafeInteger(schedule.milliseconds) || schedule.milliseconds < 0 || schedule.milliseconds > 2147483647)) throw new Error("schedule");
    const keys = new Set();
    function node(value) {
      if (!value || typeof value !== "object" || typeof value.key !== "string" || value.key.length === 0 || keys.has(value.key)) throw new Error("tree");
      keys.add(value.key);
      if (value.kind === "text") {
        if (typeof value.text !== "string") throw new Error("tree");
        return;
      }
      if (value.kind !== "element" || typeof value.tag !== "string" || value.tag.length === 0) throw new Error("tree");
      if (value.attributes === undefined) value.attributes = {};
      if (value.children === undefined) value.children = [];
      if (!value.attributes || typeof value.attributes !== "object" || Array.isArray(value.attributes) || !Array.isArray(value.children)) throw new Error("tree");
      if (Object.values(value.attributes).some(item => typeof item !== "string")) throw new Error("tree");
      for (const child of value.children) node(child);
    }
    node(output.tree);
  }
  function make(node, old, parentSvg = false) {
    let element;
    if (node.kind === "text") {
      element = old && old.nodeType === Node.TEXT_NODE ? old : document.createTextNode("");
      element.data = node.text;
    } else {
      const svg = parentSvg || node.tag === "svg";
      const namespace = svg ? "http://www.w3.org/2000/svg" : "http://www.w3.org/1999/xhtml";
      element = old && old.nodeType === Node.ELEMENT_NODE && old.localName === node.tag && old.namespaceURI === namespace
        ? old : document.createElementNS(namespace, node.tag);
      for (const attribute of [...element.attributes]) if (!(attribute.name in node.attributes)) element.removeAttribute(attribute.name);
      for (const [name,value] of Object.entries(node.attributes)) {
        if (name === "style" && "style" in element) element.style.cssText = value;
        else element.setAttribute(name,value);
      }
      const keyed = new Map([...element.childNodes].map(child => [child.__scorepeekKey, child]));
      for (const child of node.children) element.append(make(child, keyed.get(child.key), svg && node.tag !== "foreignObject"));
      for (const child of [...element.childNodes]) if (!node.children.some(next => next.key === child.__scorepeekKey)) child.remove();
    }
    if (old && old !== element) old.remove();
    element.__scorepeekKey = node.key;
    return element;
  }
  function apply(tree) { root.replaceChildren(make(tree, root.firstChild)); }

  const sample = new URLSearchParams(location.search).get("sample") === "1" ? "?sample=1" : "";
  const socket = new WebSocket(`${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/ws/${encodeURIComponent(spec.canvas.id)}${sample}`);
  function sameSpecification(candidate) { return JSON.stringify(candidate) === JSON.stringify(spec); }
  socket.onmessage = event => {
    const message = JSON.parse(event.data);
    if (message.type === "state") { state = message.state; requestRender(); }
    if (message.type === "presentation") {
      if (awaitingEditorGeometry && !sameSpecification(message.specification)) return;
      awaitingEditorGeometry = false;
      spec = message.specification;
      requestRender();
    }
    if (message.type === "canvas_unavailable") fail("canvas_unavailable");
  };
  addEventListener("message", event => {
    if (event.source !== parent || event.origin !== location.origin || typeof event.data !== "string") return;
    let message;
    try { message = JSON.parse(event.data); } catch (_) { return; }
    if (message.type !== "scorepeek-editor-presentation"
        || message.specification?.canvas?.id !== spec.canvas.id
        || !Number.isSafeInteger(message.session_id)
        || !Number.isSafeInteger(message.revision)
        || (message.session_id === lastEditorSession
            && message.revision <= lastEditorRevision)) return;
    lastEditorSession = message.session_id;
    lastEditorRevision = message.revision;
    awaitingEditorGeometry = true;
    spec = message.specification;
    requestRender();
  });
  document.addEventListener("visibilitychange", () => { if (!document.hidden) requestRender(); });
  start();
})();
