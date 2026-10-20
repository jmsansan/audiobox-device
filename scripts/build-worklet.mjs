#!/usr/bin/env node
// Builds the WebAssembly module of the audio worklet and writes it where audiobox-device
// embeds it: web/audiobox_device_worklet.wasm. Run it after changing the mixer's code,
// the protocol or the worklet crate, and commit the result.
//
//   node scripts/build-worklet.mjs [--check]
//
// --check builds into a temporary place and fails if the committed module differs, so a
// release can verify that the module matches the source.
import { execFileSync } from 'node:child_process';
import { copyFileSync, readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const check = process.argv.includes('--check');
execFileSync(
  'cargo',
  ['build', '-p', 'audiobox-device-worklet', '--release', '--locked', '--target', 'wasm32-unknown-unknown'],
  { cwd: root, stdio: 'inherit' },
);
const built = join(root, 'target/wasm32-unknown-unknown/release/audiobox_device_worklet.wasm');
const target = join(root, 'web/audiobox_device_worklet.wasm');
const bytes = readFileSync(built);

// The worklet's scope gives the module nothing, so it must import nothing.
const module = new WebAssembly.Module(bytes);
const imports = WebAssembly.Module.imports(module);
if (imports.length > 0) {
  console.error('The worklet module imports', imports);
  process.exit(1);
}
const exported = WebAssembly.Module.exports(module).map((entry) => entry.name).sort();
for (const name of ['apply', 'channels', 'input', 'memory', 'render', 'reports_clear', 'reports_len', 'reports_ptr']) {
  if (!exported.includes(name)) {
    console.error(`The worklet module does not export ${name}.`);
    process.exit(1);
  }
}

if (check) {
  const committed = readFileSync(target);
  if (!committed.equals(bytes)) {
    console.error('web/audiobox_device_worklet.wasm differs from a fresh build: run node scripts/build-worklet.mjs.');
    process.exit(1);
  }
  console.log(`web/audiobox_device_worklet.wasm matches the source (${bytes.length} bytes).`);
} else {
  copyFileSync(built, target);
  console.log(`Wrote web/audiobox_device_worklet.wasm (${bytes.length} bytes), no imports.`);
}
