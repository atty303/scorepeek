# ADR 0132: Motion-led overlay and system Japanese fonts

- Status: Accepted
- Date: 2026-09-06
- Supersedes: ADR 0131's embedded Japanese font and generous full-frame material treatment.

Keep the existing information design and content typography. Use restrained frame lines and corner
accents, with staggered moving highlights carrying the ambient visual interest. Clear-type and
status effects retain their semantic emphasis. Richness should come primarily from motion rather
than dense, continuously bright frame artwork.

Bundle the role-specific Latin fonts. Japanese uses system Noto Sans JP or the platform sans-serif
fallback; do not bundle a Japanese font or fetch one at runtime. The exact Japanese appearance may
therefore vary with fonts installed on the rendering host.

Native and browser continue to share DOM, CSS and motion settings. Enable the pinned Blitz SVG
feature for history lines. Inline SVG carries explicit paint attributes and viewport dimensions,
sharing colors with CSS legends and axes and dimensions with the plot layout.
