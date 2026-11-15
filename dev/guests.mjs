#!/usr/bin/env node
// Test guests for a call: headless Chrome joining a room's link as guests,
// each with a made-up camera (a colour of its own, something moving) and a
// recorded voice. They take turns speaking, about ten seconds each, so the
// app has speakers to follow. Their own sound output is muted: nothing they
// hear comes out of this Mac's speakers.
//
//   node dev/guests.mjs join <room link> [Name ...]   default: Anna Lee Mia
//   node dev/guests.mjs mic <Name>                    mute or unmute
//   node dev/guests.mjs camera <Name>                 camera off or on
//   node dev/guests.mjs status
//   node dev/guests.mjs leave [Name ...]              everyone, if no name
//
// The room must let guests in: a public room, or a guest invitation link
// for each. Needs Google Chrome; `say` for the voices, `ffmpeg` for the
// cameras (without it, Chrome's own test pattern). Node 22 or later.

import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const CHROME = process.env.CHROME ?? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const DIR = join(tmpdir(), 'quipcall-guests');
const STATE = join(DIR, 'state.json');
const FIRST_PORT = 9411;
/// How long each guest has the floor, and what they say in it.
const TURN = 10;
const SPEECH = 7.5;
const CAST = [
  ['Samantha', '0x2f6fdb', 'I think the new widget is a lot calmer. Nothing jumps when I move the mouse over it.'],
  ['Daniel', '0xd4573b', 'Agreed. I dragged my bubble to the corner and it stayed there. Can you see me speaking now?'],
  ['Karen', '0x2f9e63', 'My turn. I am the third guest, talking for a few seconds so the bubble follows me.'],
  ['Moira', '0x8a4fd1', 'And I am the fourth. If you hid my video, click my face to bring me back.'],
  ['Rishi', '0xc79a1e', 'Fifth guest here. With five of us the quiet ones should be in the stack.'],
  ['Tessa', '0x1f9aa8', 'Sixth and last. After me it is the first guest again.'],
];

const load = () => (existsSync(STATE) ? JSON.parse(readFileSync(STATE, 'utf8')) : {});
const save = (state) => writeFileSync(STATE, JSON.stringify(state, null, 2));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const alive = (pid) => { try { process.kill(pid, 0); return true; } catch { return false; } };
/** Whether something already answers DevTools on `port`. */
const taken = async (port) => {
  try {
    await fetch(`http://127.0.0.1:${port}/json/version`, { signal: AbortSignal.timeout(500) });
    return true;
  } catch {
    return false;
  }
};

/** A WAV's samples (16-bit mono), whatever chunks come before them. */
function samples(file) {
  const b = readFileSync(file);
  let at = 12;
  while (at + 8 <= b.length) {
    const id = b.toString('ascii', at, at + 4);
    const size = b.readUInt32LE(at + 4);
    if (id === 'data') return b.subarray(at + 8, Math.min(at + 8 + size, b.length));
    at += 8 + size + (size % 2);
  }
  throw new Error(`no audio in ${file}`);
}

/** A voice saying `text` in its turn of a loop `turns` long, silent otherwise. */
function voice(name, ix, turns, who, text) {
  const raw = join(DIR, `${name}.say.wav`);
  const out = join(DIR, `${name}.wav`);
  const said = spawnSync('say', ['-v', who, '-o', raw, '--file-format=WAVE', '--data-format=LEI16@48000', text]);
  if (said.status !== 0) return undefined;
  const rate = 48000;
  const speech = samples(raw).subarray(0, Math.floor(SPEECH * rate) * 2);
  const data = Buffer.alloc(turns * TURN * rate * 2);
  speech.copy(data, ix * TURN * rate * 2);
  const head = Buffer.alloc(44);
  head.write('RIFF', 0); head.writeUInt32LE(36 + data.length, 4); head.write('WAVEfmt ', 8);
  head.writeUInt32LE(16, 16); head.writeUInt16LE(1, 20); head.writeUInt16LE(1, 22);
  head.writeUInt32LE(rate, 24); head.writeUInt32LE(rate * 2, 28); head.writeUInt16LE(2, 32);
  head.writeUInt16LE(16, 34); head.write('data', 36); head.writeUInt32LE(data.length, 40);
  writeFileSync(out, Buffer.concat([head, data]));
  return out;
}

/** A camera: one colour, with a white square going round. */
function camera(name, colour) {
  const out = join(DIR, `${name}.mjpeg`);
  const made = spawnSync('ffmpeg', ['-y', '-loglevel', 'error',
    '-f', 'lavfi', '-i', `color=c=${colour}:s=640x480:r=15:d=8`,
    '-f', 'lavfi', '-i', 'color=c=white:s=90x90:r=15:d=8',
    '-filter_complex', "[0][1]overlay=x='275+200*sin(2*PI*t/8)':y='195+130*cos(2*PI*t/8)'",
    '-c:v', 'mjpeg', '-q:v', '6', '-pix_fmt', 'yuvj420p', out]);
  return made.status === 0 ? out : undefined;
}

/** Talk to a guest's page over the DevTools protocol. */
async function page(port) {
  let target;
  for (let i = 0; i < 50 && !target; i++) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
      target = list.find((t) => t.type === 'page');
    } catch { /* Chrome is still starting. */ }
    if (!target) await sleep(200);
  }
  if (!target) throw new Error(`no Chrome on port ${port}`);
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((ok, fail) => { ws.onopen = ok; ws.onerror = () => fail(new Error('DevTools refused')); });
  let next = 0;
  const waiting = new Map();
  ws.onmessage = (m) => {
    const msg = JSON.parse(m.data);
    waiting.get(msg.id)?.(msg);
    waiting.delete(msg.id);
  };
  return {
    async run(fn, ...args) {
      const id = ++next;
      const reply = new Promise((r) => waiting.set(id, r));
      ws.send(JSON.stringify({ id, method: 'Runtime.evaluate', params: {
        expression: `(${fn})(${args.map((a) => JSON.stringify(a)).join(',')})`,
        awaitPromise: true, returnByValue: true } }));
      const { result, error } = await Promise.race([reply,
        sleep(10000).then(() => ({ error: { message: 'the page did not answer' } }))]);
      if (error || result.exceptionDetails) {
        throw new Error(error?.message ?? result.exceptionDetails.exception?.description ?? 'page error');
      }
      return result.result.value;
    },
    close: () => ws.close(),
  };
}

// ---- what runs in the guest's page -------------------------------------

/**
 * One step towards being in the call; asked again until it says `done`.
 * Without `go` it stops at the door (`ready`), so everyone can walk in at
 * once: a guest's voice starts its loop when it joins.
 */
const STEP = (name, go) => {
  const texts = [...document.querySelectorAll('button')].map((b) => b.textContent.trim());
  if (texts.includes('Leave')) return { done: 'joined' };
  const title = document.querySelector('h1')?.textContent ?? document.title;
  if (!document.querySelector('.join-layout')) {
    // The page says "Checking your link…" first, in the same place as a refusal.
    const refused = document.querySelector('.result') && !/^Checking/.test(title);
    return refused ? { done: `not let in: ${title}` } : { at: 'checking the link' };
  }
  const error = document.querySelector('.error')?.textContent?.trim();
  const [mic, cam] = document.querySelectorAll('.media-controls button');
  for (const [button, what] of [[mic, 'microphone'], [cam, 'camera']]) {
    if (button.getAttribute('aria-pressed') === 'true') continue;
    // "Cancel … setup" while it starts: leave it be.
    if (!button.disabled && !/^Cancel/.test(button.textContent.trim())) button.click();
    return { at: `starting the ${what}`, error };
  }
  const input = document.querySelector('.join-info form input');
  if (input.value !== name) {
    input.value = name;
    input.dispatchEvent(new Event('input', { bubbles: true }));
    return { at: 'typing the name' };
  }
  const join = document.querySelector('.join-button');
  if (join.disabled) return { at: 'joining', error };
  if (!go) return { ready: true, at: 'ready to join' };
  join.click();
  return { at: 'joining', error };
};

const PRESS = (labels) => {
  const buttons = [...document.querySelectorAll('button')];
  const button = buttons.find((b) => labels.includes(b.textContent.trim()));
  if (!button || !buttons.some((b) => b.textContent.trim() === 'Leave')) return 'not in a call';
  button.click();
  return `was "${button.textContent.trim()}"`;
};

const STATUS = () => {
  const texts = [...document.querySelectorAll('button')].map((b) => b.textContent.trim());
  // The page before the call has mic and camera buttons too; only a call has Leave.
  if (!texts.includes('Leave')) return `not in a call (${document.querySelector('h1')?.textContent ?? '?'})`;
  const one = (labels) => texts.find((t) => labels.includes(t));
  return `in the call, ${one(['Mic on', 'Mic off'])}, ${one(['Camera on', 'Camera off'])}`;
};

// ---- commands ----------------------------------------------------------

async function joinAll(link, names) {
  if (!existsSync(CHROME)) throw new Error(`Google Chrome isn't at ${CHROME} (set CHROME)`);
  new URL(link);
  mkdirSync(DIR, { recursive: true });
  const state = load();
  for (const [name, guest] of Object.entries(state)) if (!alive(guest.pid)) delete state[name];
  // Later guests take the turns after the ones already here.
  const here = Object.keys(state).length;
  const turns = Math.max(here + names.length, 2);
  let port = Math.max(FIRST_PORT - 1, ...Object.values(state).map((g) => g.port)) + 1;
  const started = [];
  for (const [i, name] of names.entries()) {
    if (state[name]) {
      console.log(`${name}: already here`);
      continue;
    }
    // A Chrome left behind may still hold a port: its page isn't this guest's.
    while (await taken(port)) port += 1;
    const ix = here + i;
    const [who, colour, text] = CAST[ix % CAST.length];
    const profile = join(DIR, `profile-${name}`);
    rmSync(profile, { recursive: true, force: true });
    const audio = voice(name, ix, turns, who, text);
    const video = camera(name, colour);
    const args = [
      '--headless=new', `--user-data-dir=${profile}`, `--remote-debugging-port=${port}`,
      '--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream',
      // Chrome reads the files only without its sandbox.
      '--no-sandbox', '--mute-audio', '--no-first-run', '--disable-gpu-sandbox',
      '--autoplay-policy=no-user-gesture-required', '--window-size=1000,700',
      ...(audio ? [`--use-file-for-fake-audio-capture=${audio}`] : []),
      ...(video ? [`--use-file-for-fake-video-capture=${video}`] : []),
      link,
    ];
    const chrome = spawn(CHROME, args, { detached: true, stdio: 'ignore' });
    chrome.unref();
    state[name] = { pid: chrome.pid, port };
    started.push([name, port]);
    port += 1;
    save(state);
  }
  // Everyone to the door first, then in together, so their turns line up.
  const guests = await Promise.all(started.map(async ([name, port]) => {
    try {
      return { name, page: await page(port), last: {} };
    } catch (e) {
      return { name, last: { done: e.message } };
    }
  }));
  const walk = async (go, ms, arrived) => {
    const deadline = Date.now() + ms;
    await Promise.all(guests.map(async (guest) => {
      while (guest.page && !arrived(guest.last) && Date.now() < deadline) {
        try {
          guest.last = await guest.page.run(STEP, guest.name, go);
        } catch (e) {
          guest.last = { done: e.message };
        }
        if (!arrived(guest.last)) await sleep(300);
      }
    }));
  };
  await walk(false, 45000, (last) => last.done || last.ready);
  await walk(true, 45000, (last) => last.done);
  for (const { name, page: p, last } of guests) {
    console.log(`${name}: ${last.done ?? `stuck ${last.at}${last.error ? ` (${last.error})` : ''}`}`);
    p?.close();
  }
}

/** Run `fn` on each named guest's page (everyone's, with no names), then `after`, whatever happened. */
async function each(names, fn, after = () => {}) {
  const state = load();
  for (const name of names.length ? names : Object.keys(state)) {
    const guest = state[name];
    if (!guest || !alive(guest.pid)) {
      console.log(`${name}: not here`);
      delete state[name];
      continue;
    }
    try {
      const p = await page(guest.port);
      console.log(`${name}: ${await fn(p, guest, name)}`);
      p.close();
    } catch (e) {
      console.log(`${name}: ${e.message}`);
    } finally {
      after(guest, name, state);
    }
  }
  return state;
}

const [command, ...rest] = process.argv.slice(2);
try {
  if (command === 'join' && rest.length) {
    await joinAll(rest[0], rest.length > 1 ? rest.slice(1) : ['Anna', 'Lee', 'Mia']);
  } else if (command === 'mic' && rest.length) {
    await each(rest, (p) => p.run(PRESS, ['Mic on', 'Mic off']));
  } else if (command === 'camera' && rest.length) {
    await each(rest, (p) => p.run(PRESS, ['Camera on', 'Camera off']));
  } else if (command === 'status') {
    await each([], (p) => p.run(STATUS));
  } else if (command === 'leave') {
    // Chrome goes even if its page didn't answer: a guest dropped from the
    // list but still running would stay in the call, holding its port.
    const state = await each(rest, async (p) => {
      const said = await p.run(PRESS, ['Leave']);
      await sleep(400);
      return said === 'not in a call' ? 'closed' : 'left';
    }, (guest, name, state) => {
      try { process.kill(guest.pid); } catch { /* Already gone. */ }
      delete state[name];
    });
    save(state);
  } else {
    console.log(readFileSync(new URL(import.meta.url), 'utf8').split('\n').slice(1, 17).map((l) => l.replace(/^\/\/ ?/, '')).join('\n'));
    process.exit(command ? 1 : 0);
  }
  // Open sockets to the pages would keep Node waiting.
  process.exit(0);
} catch (e) {
  console.error(`error: ${e.message}`);
  process.exit(1);
}
