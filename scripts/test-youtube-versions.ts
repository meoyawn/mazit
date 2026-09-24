import { $ } from "bun";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve, join } from "node:path";

const root = resolve(import.meta.dir, "..");
const crate = join(root, "youtubei");
const manifest = await Bun.file(join(crate, "package.json")).json();
const bundle = join(crate, "generated/youtubei.js");
const original = await Bun.file(bundle).bytes();
const versions = process.argv.slice(2);
const directory = await mkdtemp(join(tmpdir(), "mazit-youtube-versions-"));

try {
  for (const version of versions.length ? versions : ["18.0.0", "18.1.0"]) {
    await Bun.write(
      join(directory, "package.json"),
      JSON.stringify({
        ...manifest,
        dependencies: { "youtubei.js": version },
      }),
    );
    await $`bun install`.cwd(directory);
    const installed = await Bun.file(
      join(directory, "node_modules/youtubei.js/package.json"),
    ).json();
    console.log(`Testing youtubei.js ${installed.version}`);
    await $`bun run bundle`.cwd(directory);
    await Bun.write(bundle, Bun.file(join(directory, "generated/youtubei.js")));
    await $`cargo test --locked`.cwd(crate);
    await $`cargo test --locked --lib youtube::`.cwd(root);
  }
} finally {
  await Bun.write(bundle, original);
  await rm(directory, { recursive: true, force: true });
}
