// Create a full-function baseline skin in the repository without reusing a bundled skin.
// Usage: deno run --allow-read --allow-write scaffold-skin.ts REPOSITORY_ROOT SLUG ID NAME AUTHOR
import { basename, join } from "node:path";

const [root, slug, id, name, author] = Deno.args;
if (!root || !slug || !id || !name || !author || Deno.args.length !== 5) {
  throw new Error("usage: scaffold-skin.ts REPOSITORY_ROOT SLUG ID NAME AUTHOR");
}
if (!/^[a-z][a-z0-9-]*$/.test(slug) ||
  !/^[a-z][a-z0-9-]*(\.[a-z][a-z0-9-]*){2,}$/.test(id)) {
  throw new Error("slug or reverse-domain skin ID is invalid");
}
if ([name, author].some((value) => /[\r\n"]/.test(value))) {
  throw new Error("name and author must be single-line TOML strings");
}
const skill = join(root, ".agents/skills/create-overlay-skin");
const source = join(skill, "starter");
const target = join(root, "skins", slug);
const workspacePath = join(root, "Cargo.toml");
const workspace = await Deno.readTextFile(workspacePath);
const member = `"skins/${slug}"`;
const match = workspace.match(/^members = \[(.*)\]$/m);
if (!match || match[1].includes(member)) {
  throw new Error("workspace members are unsupported or already include this skin");
}
if (basename(target) !== slug) throw new Error("invalid target directory");
await Deno.mkdir(target); // Existing directories, including partial work, are never overwritten.
try {
  await Deno.mkdir(join(target, "src"));
  await Deno.mkdir(join(target, "resources"));
  for (const file of ["src/lib.rs", "src/extras.rs", "src/metrics.rs", "theme.css", "preview.png", "resources/scorepeek-logo-dark-transparent.png"]) {
    await Deno.copyFile(join(source, file), join(target, file));
  }
  const cargo = (await Deno.readTextFile(join(source, "Cargo.toml")))
    .replace("scorepeek-skin-starter", `scorepeek-skin-${slug}`);
  const manifest = (await Deno.readTextFile(join(source, "skin.toml")))
    .replace("dev.example.scorepeek.skin.starter", id)
    .replace('name = "Starter Skin"', `name = "${name}"`)
    .replace('author = "Your name"', `author = "${author}"`);
  await Deno.writeTextFile(join(target, "Cargo.toml"), cargo);
  await Deno.writeTextFile(join(target, "skin.toml"), manifest);
  await Deno.writeTextFile(
    workspacePath,
    workspace.replace(match[0], `members = [${match[1]}, ${member}]`),
  );
} catch (error) {
  // A failed scaffold is left intact for inspection; no existing user files are removed.
  throw error;
}
console.log(target);
