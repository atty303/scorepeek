(() => {
  const tracks = JSON.parse(document.getElementById('scorepeek-motion').textContent);
  let frame;
  const start = performance.now();
  const tick = now => {
    const seconds = (now - start) / 1000;
    for (const track of tracks) {
      const phase = ((seconds / track.period + track.phase) % 1 + 1) % 1;
      const blend = track.wave === 'sine' ? .5 - .5 * Math.cos(phase * Math.PI * 2) : phase;
      const value = (track.from + (track.to - track.from) * blend).toFixed(4) + track.unit;
      for (const node of document.querySelectorAll(track.selector)) node.style.setProperty(track.property, value);
    }
    frame = requestAnimationFrame(tick);
  };
  const resume = () => {
    cancelAnimationFrame(frame);
    if (!document.hidden) frame = requestAnimationFrame(tick);
  };
  document.addEventListener('visibilitychange', resume);
  window.addEventListener('pagehide', () => cancelAnimationFrame(frame));
  window.addEventListener('pageshow', resume);
  resume();
})();
