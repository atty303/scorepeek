// Add discovered repository skins to the neutral, versioned preview scene.
// Usage: deno run --allow-read --allow-write prepare-preview-scene.ts TEMPLATE OUTPUT SKINS_DIRECTORY [SLUG]
const [templatePath, outputPath, skinsDirectory, requestedSlug] = Deno.args;
if (!templatePath || !outputPath || !skinsDirectory || Deno.args.length > 4) {
  throw new Error(
    "usage: prepare-preview-scene.ts TEMPLATE OUTPUT SKINS_DIRECTORY [SLUG]",
  );
}
const scene = JSON.parse(await Deno.readTextFile(templatePath));
if (scene.schema !== "scorepeek-skin-preview-scene-v1") {
  throw new Error("unsupported preview scene");
}
const skins = [];
for await (const entry of Deno.readDir(skinsDirectory)) {
  if (!entry.isDirectory || (requestedSlug && entry.name !== requestedSlug)) {
    continue;
  }
  const manifestPath = `${skinsDirectory}/${entry.name}/skin.toml`;
  let source;
  try {
    source = await Deno.readTextFile(manifestPath);
  } catch (error) {
    if (error instanceof Deno.errors.NotFound) continue;
    throw error;
  }
  const id = source.match(/^id\s*=\s*"([^"]+)"/m)?.[1];
  if (!id) throw new Error(`missing skin id: ${manifestPath}`);
  skins.push({ slug: entry.name, id });
}
skins.sort((a, b) => a.slug.localeCompare(b.slug));
if (skins.length === 0) throw new Error(`no skins found: ${requestedSlug ?? skinsDirectory}`);
scene.skins = skins;
await Deno.writeTextFile(outputPath, JSON.stringify(scene));
