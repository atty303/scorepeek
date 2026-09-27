// Locate a rendered widget by the rectangle supplied to the skin API.
// Skin authors are free to choose their own DOM tags, classes, and attributes.
export async function widgetRoot(page, widget) {
  return await page.evaluateHandle(({ x, y, width, height }) => {
    const near = (actual, expected) => Math.abs(actual - expected) <= 1;
    for (const element of document.body.querySelectorAll("*")) {
      const rect = element.getBoundingClientRect();
      if (
        rect.width > 0 && rect.height > 0 &&
        near(rect.x, x) && near(rect.y, y) &&
        near(rect.width, width) && near(rect.height, height) &&
        getComputedStyle(element).visibility !== "hidden"
      ) return element;
    }
    return null;
  }, widget);
}

export async function widgetText(page, widget) {
  const root = await widgetRoot(page, widget);
  try {
    return await root.evaluate((element) => element?.textContent ?? null);
  } finally {
    await root.dispose();
  }
}

// Text in the DOM can extend past an overflow clip or be replaced by an
// ellipsis. Test glyph ranges against the widget and every clipping ancestor.
export async function widgetTextVisibility(page, widget, value) {
  const root = await widgetRoot(page, widget);
  try {
    return await root.evaluate((element, expected) => {
      if (!element) return { visible: false, reason: "widget root missing" };
      const nodes = [];
      const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT);
      let text = "";
      while (walker.nextNode()) {
        const node = walker.currentNode;
        nodes.push({
          node,
          start: text.length,
          end: text.length + node.length,
        });
        text += node.textContent;
      }
      const point = (offset, endBoundary = false) => {
        const entry = nodes.find(({ start, end }) =>
          endBoundary
            ? start < offset && offset <= end
            : start <= offset && offset < end
        ) ?? nodes.at(-1);
        return [entry.node, offset - entry.start];
      };
      const clips = (node) => {
        const result = [];
        for (let current = node; current; current = current.parentElement) {
          const style = getComputedStyle(current);
          if (
            style.display === "none" || Number(style.opacity) === 0 ||
            (current === node && style.visibility !== "visible")
          ) {
            return null;
          }
          const x = current === element ||
            /^(hidden|clip|scroll|auto)$/.test(style.overflowX);
          const y = current === element ||
            /^(hidden|clip|scroll|auto)$/.test(style.overflowY);
          if (x || y) {
            const rect = current.getBoundingClientRect();
            result.push({
              x,
              y,
              left: rect.left + current.clientLeft,
              right: rect.left + current.clientLeft + current.clientWidth,
              top: rect.top + current.clientTop,
              bottom: rect.top + current.clientTop + current.clientHeight,
            });
          }
          if (current === element) {
            break;
          }
        }
        return result;
      };
      if (!expected || !text.includes(expected)) {
        return { visible: false, reason: "text missing from widget" };
      }
      const segmenter = new Intl.Segmenter(undefined, {
        granularity: "grapheme",
      });
      let failure;
      for (
        let start = text.indexOf(expected);
        start !== -1;
        start = text.indexOf(expected, start + 1)
      ) {
        let candidateFailure;
        for (const { segment, index } of segmenter.segment(expected)) {
          if (/^\s+$/u.test(segment)) {
            continue;
          }
          const [startNode, startOffset] = point(start + index);
          const [endNode, endOffset] = point(
            start + index + segment.length,
            true,
          );
          const range = document.createRange();
          range.setStart(startNode, startOffset);
          range.setEnd(endNode, endOffset);
          const bounds = clips(startNode.parentElement);
          const rects = [...range.getClientRects()];
          if (!bounds || rects.length === 0) {
            candidateFailure = {
              visible: false,
              reason: "text is hidden",
              segment,
            };
            break;
          }
          // Font metrics can extend a few pixels beyond the painted glyphs.
          const verticalSlack = Math.max(
            1,
            Number.parseFloat(
              getComputedStyle(startNode.parentElement).fontSize,
            ) /
              4,
          );
          const outside = rects.find((rect) =>
            bounds.some((clip) =>
              (clip.x &&
                (rect.left < clip.left - 1 || rect.right > clip.right + 1)) ||
              (clip.y &&
                (rect.top < clip.top - verticalSlack ||
                  rect.bottom > clip.bottom + verticalSlack))
            )
          );
          if (outside) {
            candidateFailure = {
              visible: false,
              reason: "text is clipped",
              segment,
              rect: {
                x: outside.x,
                y: outside.y,
                width: outside.width,
                height: outside.height,
              },
            };
            break;
          }
        }
        if (!candidateFailure) {
          return { visible: true };
        }
        failure ??= candidateFailure;
      }
      return failure;
    }, String(value));
  } finally {
    await root.dispose();
  }
}

export async function widgetRect(page, widget) {
  const root = await widgetRoot(page, widget);
  try {
    return await root.evaluate((element) => {
      if (!element) return null;
      const rect = element.getBoundingClientRect();
      return { x: rect.x, y: rect.y, width: rect.width, height: rect.height };
    });
  } finally {
    await root.dispose();
  }
}

// The review board reuses case 01's canvas behind every variant. Hide every
// widget to check that a backdrop-dependent variant sees that same canvas.
export async function captureCanvasBackdrop(page, widgets) {
  const saved = await page.evaluate((widgets) => {
    const near = (actual, expected) => Math.abs(actual - expected) <= 1;
    const elements = [...document.body.querySelectorAll("*")];
    return widgets.map((widget) => {
      const element = elements.find((candidate) => {
        const rect = candidate.getBoundingClientRect();
        return near(rect.x, widget.x) && near(rect.y, widget.y) &&
          near(rect.width, widget.width) && near(rect.height, widget.height);
      });
      if (!element) throw new Error(`widget root missing: ${widget.id}`);
      const value = element.style.getPropertyValue("visibility");
      const priority = element.style.getPropertyPriority("visibility");
      element.style.setProperty("visibility", "hidden", "important");
      return { widget, value, priority };
    });
  }, widgets);
  try {
    return await page.screenshot({ omitBackground: true });
  } finally {
    await page.evaluate((saved) => {
      const near = (actual, expected) => Math.abs(actual - expected) <= 1;
      const elements = [...document.body.querySelectorAll("*")];
      for (const entry of saved) {
        const widget = entry.widget;
        const element = elements.find((candidate) => {
          const rect = candidate.getBoundingClientRect();
          return near(rect.x, widget.x) && near(rect.y, widget.y) &&
            near(rect.width, widget.width) && near(rect.height, widget.height);
        });
        if (!element) throw new Error(`widget root missing: ${widget.id}`);
        if (entry.value) {
          element.style.setProperty("visibility", entry.value, entry.priority);
        } else element.style.removeProperty("visibility");
      }
    }, saved);
  }
}

export async function backdropDigest(image) {
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", image));
  return [...digest].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

// Capture only this widget's paint onto transparency. A full rectangular crop
// would copy unrelated widgets from the variant scene into the review board.
export async function captureWidgetOnly(
  page,
  widget,
  clip,
  outputPath,
  verifyIsolation = false,
  backdropContext = null,
) {
  const root = await widgetRoot(page, widget);
  if (!(await root.evaluate((element) => element !== null))) {
    await root.dispose();
    throw new Error(`widget root missing: ${widget.id}`);
  }
  const dependsOnBackdrop = await root.evaluate((element) => {
    const usesBackdrop = (node) => {
      for (const pseudo of [null, "::before", "::after"]) {
        const style = getComputedStyle(node, pseudo);
        if (
          style.mixBlendMode !== "normal" ||
          (style.backdropFilter && style.backdropFilter !== "none") ||
          (style.webkitBackdropFilter && style.webkitBackdropFilter !== "none")
        ) return true;
      }
      return false;
    };
    for (let node = element; node; node = node.parentElement) {
      if (usesBackdrop(node)) return true;
    }
    return [...element.querySelectorAll("*")].some(usesBackdrop);
  });
  if (dependsOnBackdrop) {
    if (!backdropContext) {
      await root.dispose();
      throw new Error(`${widget.id}: backdrop reference is required`);
    }
    const currentBackdrop = await captureCanvasBackdrop(
      page,
      backdropContext.widgets,
    );
    const reference = (await Deno.readTextFile(backdropContext.referencePath))
      .trim();
    if (await backdropDigest(currentBackdrop) !== reference) {
      await root.dispose();
      throw new Error(
        `${widget.id}: canvas backdrop differs from case 01 at this frame; ` +
          "the one-board review cannot faithfully composite backdrop effects",
      );
    }
    // Preserve the production backdrop, then mask out pixels unaffected by the
    // widget. The browser review clock is frozen during these two captures.
    const shown = await page.screenshot({ omitBackground: true, clip });
    const savedVisibility = await root.evaluate((element) => [
      element.style.getPropertyValue("visibility"),
      element.style.getPropertyPriority("visibility"),
    ]);
    let hidden;
    try {
      await root.evaluate((element) => {
        element.style.setProperty("visibility", "hidden", "important");
      });
      hidden = await page.screenshot({ omitBackground: true, clip });
    } finally {
      await root.evaluate((element, [value, priority]) => {
        if (value) element.style.setProperty("visibility", value, priority);
        else element.style.removeProperty("visibility");
      }, savedVisibility);
      await root.dispose();
    }
    const base64 = await page.evaluate(
      async ({ shown, hidden, width, height }) => {
        async function pixels(image) {
          const bytes = Uint8Array.from(
            atob(image),
            (char) => char.charCodeAt(0),
          );
          const bitmap = await createImageBitmap(
            new Blob([bytes], { type: "image/png" }),
          );
          const canvas = document.createElement("canvas");
          canvas.width = width;
          canvas.height = height;
          const context = canvas.getContext("2d", { willReadFrequently: true });
          context.drawImage(bitmap, 0, 0);
          bitmap.close();
          return context.getImageData(0, 0, width, height).data;
        }
        const before = await pixels(shown);
        const without = await pixels(hidden);
        const canvas = document.createElement("canvas");
        canvas.width = width;
        canvas.height = height;
        const context = canvas.getContext("2d");
        const output = context.createImageData(width, height);
        let ambiguousPixels = 0;
        for (let index = 0; index < before.length; index += 4) {
          if (
            [0, 1, 2, 3].some((channel) =>
              Math.abs(before[index + channel] - without[index + channel]) > 2
            )
          ) {
            const backgroundAlpha = without[index + 3];
            const shownAlpha = before[index + 3];
            if (
              (backgroundAlpha !== 0 && backgroundAlpha !== 255) ||
              (backgroundAlpha === 255 && shownAlpha !== 255)
            ) {
              ambiguousPixels += 1;
              continue;
            }
            output.data.set(before.subarray(index, index + 3), index);
            output.data[index + 3] = backgroundAlpha === 0 ? shownAlpha : 255;
          }
        }
        if (ambiguousPixels > 0) {
          throw new Error(
            `${ambiguousPixels} backdrop pixels have partial alpha; ` +
              "the one-board review cannot reconstruct them faithfully",
          );
        }
        context.putImageData(output, 0, 0);
        return canvas.toDataURL("image/png").split(",")[1];
      },
      {
        shown: shown.toString("base64"),
        hidden: hidden.toString("base64"),
        width: clip.width,
        height: clip.height,
      },
    );
    const image = Uint8Array.from(atob(base64), (char) => char.charCodeAt(0));
    await Deno.writeFile(outputPath, image);
    return image;
  }
  const saved = await root.evaluate((element) => {
    const properties = [
      "visibility",
      "background",
      "background-color",
      "background-image",
    ];
    const remember = (node) =>
      Object.fromEntries(properties.map((name) => [
        name,
        [
          node.style.getPropertyValue(name),
          node.style.getPropertyPriority(name),
        ],
      ]));
    const original = {
      html: remember(document.documentElement),
      body: remember(document.body),
      root: remember(element),
    };
    document.documentElement.style.setProperty(
      "background",
      "transparent",
      "important",
    );
    document.body.style.setProperty("background", "transparent", "important");
    document.body.style.setProperty("visibility", "hidden", "important");
    element.style.setProperty("visibility", "visible", "important");
    return original;
  });
  try {
    const image = await page.screenshot({
      path: outputPath,
      omitBackground: true,
      clip,
    });
    if (verifyIsolation) {
      await root.evaluate((element) => {
        element.style.setProperty("visibility", "hidden", "important");
      });
      const blank = await page.screenshot({ omitBackground: true, clip });
      await root.evaluate((element) => {
        element.style.setProperty("visibility", "visible", "important");
      });
      const painted = await page.evaluate(async (image) => {
        const bytes = Uint8Array.from(
          atob(image),
          (char) => char.charCodeAt(0),
        );
        const bitmap = await createImageBitmap(
          new Blob([bytes], { type: "image/png" }),
        );
        const canvas = document.createElement("canvas");
        canvas.width = bitmap.width;
        canvas.height = bitmap.height;
        const context = canvas.getContext("2d", { willReadFrequently: true });
        context.drawImage(bitmap, 0, 0);
        bitmap.close();
        const pixels =
          context.getImageData(0, 0, canvas.width, canvas.height).data;
        let count = 0;
        for (let index = 3; index < pixels.length; index += 4) {
          if (pixels[index] > 1) count += 1;
        }
        return count;
      }, blank.toString("base64"));
      if (painted > 0) {
        throw new Error(
          `${widget.id}: ${painted} pixels remain without target`,
        );
      }
    }
    return image;
  } finally {
    await root.evaluate((element, original) => {
      for (
        const [node, values] of [
          [document.documentElement, original.html],
          [document.body, original.body],
          [element, original.root],
        ]
      ) {
        for (const [name, [value, priority]] of Object.entries(values)) {
          if (value) node.style.setProperty(name, value, priority);
          else node.style.removeProperty(name);
        }
      }
    }, saved);
    await root.dispose();
  }
}
