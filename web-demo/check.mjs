#!/usr/bin/env node
// Checks the browser output in a real browser: builds the demo page, serves it, opens it
// in a headless Chromium-based browser with a profile of its own, and waits until the
// page reports that audio runs, the worklet renders, an analyser hears the music, and the
// jump effects end.
//
//   node web-demo/check.mjs [--browser <path to Edge or Chrome>] [--no-build] [--seconds 30]
//
// It needs the wasm32-unknown-unknown target and the wasm-bindgen CLI of the version that
// Cargo.lock holds. The browser's autoplay policy is relaxed, which stands in for the
// user's first click.
import { execFileSync, spawn } from 'node:child_process';
import { createServer } from 'node:http';
import { mkdtempSync, readFileSync, rmSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, extname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const args = process.argv.slice(2);
const option = (name, fallback) => (args.includes(name) ? args[args.indexOf(name) + 1] : fallback);
const browsers = [
  'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe',
  'C:/Program Files/Google/Chrome/Application/chrome.exe',
  '/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge',
  '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
  '/usr/bin/google-chrome',
  '/usr/bin/chromium',
];
const browser = option('--browser', browsers.find((path) => existsSync(path)));
const seconds = Number(option('--seconds', '30'));
const web = join(root, 'web-demo/web');

if (!args.includes('--no-build')) {
  execFileSync('cargo', ['build', '-p', 'audiobox-device-web-demo', '--release', '--locked', '--target', 'wasm32-unknown-unknown'], { cwd: root, stdio: 'inherit' });
  execFileSync('wasm-bindgen', ['--target', 'web', '--out-dir', join(web, 'pkg'), join(root, 'target/wasm32-unknown-unknown/release/audiobox_device_web_demo.wasm')], { stdio: 'inherit' });
}

const types = { '.html': 'text/html', '.js': 'text/javascript', '.wasm': 'application/wasm' };
const server = createServer((request, response) => {
  const path = new URL(request.url, 'http://localhost').pathname;
  const file = join(web, path === '/' ? 'index.html' : path);
  if (!file.startsWith(web) || !existsSync(file)) return response.writeHead(404).end();
  response.writeHead(200, { 'content-type': types[extname(file)] ?? 'application/octet-stream' }).end(readFileSync(file));
});
await new Promise((done) => server.listen(0, '127.0.0.1', done));
const page = `http://127.0.0.1:${server.address().port}/?auto`;

const profile = mkdtempSync(join(tmpdir(), 'audiobox-device-check-'));
const debugging = 9400 + Math.floor(Math.random() * 400);
const child = spawn(browser, [
  '--headless=new', `--remote-debugging-port=${debugging}`, `--user-data-dir=${profile}`,
  '--no-first-run', '--no-default-browser-check', '--autoplay-policy=no-user-gesture-required',
  'about:blank',
], { stdio: 'ignore' });
const sleep = (ms) => new Promise((done) => setTimeout(done, ms));
let code = 1;
try {
  let target;
  for (let i = 0; i < 50 && !target; i++) {
    await sleep(200);
    try {
      target = (await (await fetch(`http://127.0.0.1:${debugging}/json/list`)).json()).find((t) => t.type === 'page');
    } catch {}
  }
  if (!target) throw new Error(`the browser did not start: ${browser}`);
  const socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((done) => socket.addEventListener('open', done));
  let next = 1;
  const pending = new Map();
  const errors = [];
  socket.addEventListener('message', (event) => {
    const message = JSON.parse(event.data);
    if (message.id && pending.has(message.id)) { pending.get(message.id)(message); pending.delete(message.id); }
    if (message.method === 'Runtime.exceptionThrown') errors.push(message.params.exceptionDetails.exception?.description ?? message.params.exceptionDetails.text);
    if (message.method === 'Runtime.consoleAPICalled' && message.params.type === 'error') errors.push(message.params.args.map((a) => a.value ?? a.description).join(' '));
  });
  const send = (method, params = {}) => new Promise((done) => { const id = next++; pending.set(id, done); socket.send(JSON.stringify({ id, method, params })); });
  await send('Runtime.enable');
  await send('Page.enable');
  await send('Page.navigate', { url: page });
  const start = Date.now();
  let status = {};
  let text = '';
  while (Date.now() - start < seconds * 1000) {
    await sleep(500);
    const result = await send('Runtime.evaluate', { expression: "document.getElementById('status')?.textContent ?? ''", returnByValue: true });
    text = result.result?.result?.value ?? '';
    try { status = JSON.parse(text); } catch { status = {}; }
    if (status.running && status.frame >= 2 * 48_000 && status.rms > 0.01 && status.ended >= 2 && status.busyEnded) break;
  }
  const passed = status.running === true && status.frame >= 2 * 48_000 && status.rms > 0.01 && status.ended >= 2 && status.busyEnded === true && status.failed?.length === 0 && errors.length === 0;
  console.log(JSON.stringify({ passed, browser, seconds: (Date.now() - start) / 1000, status, text: passed ? undefined : text, errors }, null, 1));
  code = passed ? 0 : 1;
} catch (error) {
  console.error(String(error));
  code = 2;
} finally {
  child.kill();
  server.close();
  setTimeout(() => { try { rmSync(profile, { recursive: true, force: true }); } catch {} process.exit(code); }, 1500);
}
