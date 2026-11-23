import '@fontsource-variable/bricolage-grotesque/standard.css';
import '@fontsource/instrument-serif/400-italic.css';
import '@fontsource/jetbrains-mono/400.css';
import '@fontsource/jetbrains-mono/500.css';
import '@fontsource/jetbrains-mono/600.css';
import './style.css';
import { drawVariant } from '@markon/voice-core/draw';
import { CoreEngine, type CoreState } from '@markon/voice-core/engine';
import { featuresFrom } from './features';

function $<T extends Element>(sel: string, root: ParentNode = document): T {
  const el = root.querySelector<T>(sel);
  if (!el) throw new Error(`missing ${sel}`);
  return el;
}
const $$ = <T extends Element>(sel: string, root: ParentNode = document) => [
  ...root.querySelectorAll<T>(sel),
];

const reduced = matchMedia('(prefers-reduced-motion: reduce)');
const body = document.body;

/* ---------- the core: the real Core Prime engine from @markon/voice-core ---------- */

const engine = new CoreEngine({ seed: 7, demo: true });
const canvas = $<HTMLCanvasElement>('#core');
const ctx = canvas.getContext('2d');
let W = 0;

const applyMotion = () => {
  engine.reducedMotion = reduced.matches;
};
applyMotion();
reduced.addEventListener('change', applyMotion);

new ResizeObserver(([entry]) => {
  if (!entry) return;
  W = Math.round(entry.contentRect.width * Math.min(devicePixelRatio || 1, 2));
  canvas.width = W;
  canvas.height = W;
}).observe(canvas);

/* ---------- microphone: the visitor's own voice drives the listening core ---------- */

interface Mic {
  ac: AudioContext;
  an: AnalyserNode;
  stream: MediaStream;
  time: Float32Array<ArrayBuffer>;
  freq: Uint8Array<ArrayBuffer>;
}
let mic: Mic | null = null;

let last = performance.now();
requestAnimationFrame(function frame(now) {
  if (mic) {
    mic.an.getFloatTimeDomainData(mic.time);
    mic.an.getByteFrequencyData(mic.freq);
    engine.pushFeatures(featuresFrom(mic.time, mic.freq, mic.ac.sampleRate));
  }
  engine.advance((now - last) / 1000);
  last = now;
  if (ctx && W) drawVariant(engine, ctx, W, 'prime');
  requestAnimationFrame(frame);
});

let sceneState: CoreState = 'idle';
let sceneLabel = 'Standby';
let override: CoreState | null = null;
const hudLabel = $<HTMLElement>('#hud-label');
function setCore(state: CoreState) {
  engine.setState(state);
  body.dataset.state = state;
  hudLabel.textContent = state === sceneState ? sceneLabel : state;
}
const restoreCore = () => setCore(override ?? sceneState);
restoreCore();

const talk = $<HTMLButtonElement>('#talk');
const talkLabel = $<HTMLElement>('.talk-label', talk);
const talkNote = $<HTMLElement>('#talk-note');

async function startMic() {
  try {
    const stream = await navigator.mediaDevices.getUserMedia({
      audio: { echoCancellation: true, noiseSuppression: true },
    });
    const ac = new AudioContext();
    const an = ac.createAnalyser();
    an.fftSize = 2048;
    an.smoothingTimeConstant = 0.5;
    ac.createMediaStreamSource(stream).connect(an);
    mic = { ac, an, stream, time: new Float32Array(an.fftSize), freq: new Uint8Array(an.frequencyBinCount) };
    override = 'listening';
    restoreCore();
    talk.setAttribute('aria-pressed', 'true');
    talkLabel.textContent = 'Listening · tap to stop';
    talkNote.textContent = 'Speak. The rings are your voice, analysed in this tab.';
  } catch {
    talkNote.textContent = 'The microphone is blocked. Allow it for this page to try.';
  }
}
function stopMic() {
  if (!mic) return;
  for (const t of mic.stream.getTracks()) t.stop();
  void mic.ac.close();
  mic = null;
  override = null;
  restoreCore();
  talk.setAttribute('aria-pressed', 'false');
  talkLabel.textContent = 'Talk to the core';
  talkNote.textContent = 'Your voice stays in this tab. Nothing is recorded.';
}
talk.addEventListener('click', () => (mic ? stopMic() : void startMic()));
addEventListener('keydown', (e) => {
  if (e.key === 'Escape') stopMic();
});

/* ---------- scenes: whichever section crosses the middle sets the core's state ---------- */

const hudScene = $<HTMLElement>('#hud-scene');
const navLinks = $$<HTMLAnchorElement>('.bar nav a');
const enterHooks: Record<string, () => void> = {};

const sceneIO = new IntersectionObserver(
  (entries) => {
    for (const e of entries) {
      if (!e.isIntersecting) continue;
      const s = e.target as HTMLElement;
      const id = s.id || (s.classList.contains('hero') ? 'hero' : 'numbers');
      if (body.dataset.scene === id) continue;
      body.dataset.scene = id;
      sceneState = (s.dataset.state as CoreState | undefined) ?? 'idle';
      sceneLabel = s.dataset.label ?? '';
      restoreCore();
      const index = s.querySelector('.index b')?.textContent;
      const name = s.querySelector('.index')?.lastChild?.textContent?.trim();
      hudScene.textContent =
        index && name ? `${index} · ${name}` : id === 'hero' ? '00 · Top' : (s.dataset.label ?? '');
      for (const a of navLinks) a.setAttribute('aria-current', String(a.hash === `#${id}`));
      enterHooks[id]?.();
    }
  },
  { rootMargin: '-49% 0px -50% 0px' },
);
for (const s of $$('.scene')) sceneIO.observe(s);

/* ---------- reveals and counters ---------- */

function countUp(el: HTMLElement) {
  const to = Number(el.dataset.count);
  const dec = Number(el.dataset.dec ?? 0);
  if (reduced.matches || !Number.isFinite(to)) return;
  const t0 = performance.now();
  const tick = (now: number) => {
    const p = Math.min(1, (now - t0) / 1400);
    el.textContent = (to * (1 - (1 - p) ** 4)).toFixed(dec);
    if (p < 1) requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
}

const revealIO = new IntersectionObserver(
  (entries) => {
    for (const e of entries) {
      if (!e.isIntersecting) continue;
      e.target.classList.add('is-in');
      for (const n of $$<HTMLElement>('[data-count]', e.target)) countUp(n);
      revealIO.unobserve(e.target);
    }
  },
  { threshold: 0.18 },
);
for (const group of $$('.scene')) {
  $$<HTMLElement>('[data-reveal]', group).forEach((el, i) => {
    el.style.setProperty('--d', `${i * 0.12}s`);
    revealIO.observe(el);
  });
}
revealIO.observe($('.get'));
void document.fonts.ready.then(() => body.classList.add('ready'));

/* ---------- the name follows the voice ---------- */

let voiceName = 'Markon';
for (const r of $$<HTMLInputElement>('input[name="voice"]')) {
  r.addEventListener('change', () => {
    voiceName = r.value;
    for (const n of $$('[data-name]')) n.textContent = voiceName;
  });
}

/* ---------- 01 hear: partial transcripts settle into the final one ---------- */

const typed = $<HTMLElement>('.typed');
let typeRun = 0;
enterHooks.hear = () => {
  const run = ++typeRun;
  const partials = [
    'Hey',
    `Hey ${voiceName.slice(0, 4)}`,
    `Hey ${voiceName.slice(0, 4)} on what`,
    `Hey ${voiceName}, what did`,
    `Hey ${voiceName}, what did I miss`,
    `Hey ${voiceName}, what did I miss this morning?`,
  ];
  if (reduced.matches) {
    typed.textContent = `“${partials.at(-1)}”`;
    return;
  }
  partials.forEach((p, i) => {
    setTimeout(
      () => {
        if (run !== typeRun) return;
        typed.textContent = `“${p}${i === partials.length - 1 ? '”' : ''}`;
      },
      300 + i * 340,
    );
  });
};

/* ---------- 02 speak: talk over it ---------- */

const bargeText = $<HTMLElement>('#barge-text');
const bargeState = $<HTMLElement>('#barge-state');
const bargeBtn = $<HTMLButtonElement>('#barge');
const LINE =
  'Your nine o’clock moved to ten. Sara is running late, and the build on main passed. Want me to read what Sara wrote?';
const words = LINE.split(' ').map((w) => {
  const s = document.createElement('span');
  s.textContent = `${w} `;
  bargeText.append(s);
  return s;
});
let wordAt = 0;
let bargeTimer = 0;
function speakNext() {
  if (body.dataset.scene !== 'speak') return;
  const w = words[wordAt];
  if (w) {
    w.classList.add('on');
    wordAt++;
    bargeTimer = window.setTimeout(speakNext, 170 + (w.textContent?.length ?? 4) * 22);
  } else bargeTimer = window.setTimeout(restartLine, 2200);
}
function restartLine() {
  clearTimeout(bargeTimer);
  for (const w of words) w.className = '';
  wordAt = 0;
  bargeState.textContent = 'live';
  bargeState.classList.remove('stopped');
  bargeBtn.disabled = false;
  restoreCore();
  if (reduced.matches) {
    for (const w of words) w.classList.add('on');
    return;
  }
  speakNext();
}
bargeBtn.addEventListener('click', () => {
  clearTimeout(bargeTimer);
  for (const w of words.slice(wordAt)) w.classList.add('cut');
  bargeState.textContent = 'stopped · listening';
  bargeState.classList.add('stopped');
  bargeBtn.disabled = true;
  if (!override) setCore('listening');
  bargeTimer = window.setTimeout(restartLine, 2600);
});
enterHooks.speak = restartLine;

/* ---------- 04 act: the approval card ---------- */

const approve = $<HTMLElement>('#approve');
const approveOut = $<HTMLElement>('.approve-out', approve);
let approveTimer = 0;
for (const b of $$<HTMLButtonElement>('[data-answer]', approve)) {
  b.addEventListener('click', () => {
    const yes = b.dataset.answer === 'yes';
    const time = new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    approve.dataset.phase = yes ? 'sent' : 'stopped';
    approveOut.textContent = yes
      ? `Sent at ${time}. It is in the activity log.`
      : 'Stopped. Nothing was sent, and the rest of the turn is dropped.';
    clearTimeout(approveTimer);
    approveTimer = window.setTimeout(() => {
      approve.dataset.phase = 'ask';
      approveOut.textContent = '';
    }, 5200);
  });
}

/* ---------- 05 know: sources answer one by one around the core ---------- */

const SVGNS = 'http://www.w3.org/2000/svg';
const nodesG = $<SVGGElement>('.orbit-nodes');
const srcItems = $$<HTMLLIElement>('#sources li');
const svgEl = (tag: string, attrs: Record<string, string | number>) => {
  const el = document.createElementNS(SVGNS, tag);
  for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, String(v));
  return el;
};
const nodes = srcItems.map((_, i) => {
  const a = -Math.PI / 2 + (i / srcItems.length) * Math.PI * 2;
  const cos = Math.cos(a);
  const sin = Math.sin(a);
  const g = svgEl('g', {});
  g.append(
    svgEl('line', { x1: cos * 330, y1: sin * 330, x2: cos * 430, y2: sin * 430 }),
    svgEl('circle', { cx: cos * 430, cy: sin * 430, r: 7 }),
  );
  nodesG.append(g);
  return g;
});
let srcAt = 0;
let srcHold = false;
function lightSource(i: number) {
  srcAt = i;
  for (const [k, n] of nodes.entries()) n.classList.toggle('on', k === i);
  for (const [k, n] of srcItems.entries()) n.classList.toggle('on', k === i);
}
srcItems.forEach((li, i) => {
  li.addEventListener('pointerenter', () => {
    srcHold = true;
    lightSource(i);
  });
  li.addEventListener('pointerleave', () => {
    srcHold = false;
  });
});
setInterval(() => {
  if (srcHold || body.dataset.scene !== 'know' || reduced.matches) return;
  lightSource((srcAt + 1) % nodes.length);
}, 1100);
lightSource(0);

/* ---------- 07 brain: model routes ---------- */

interface Route {
  name: string;
  tag: string;
  dest: string;
  copy: string;
  local?: boolean;
}
const ROUTES: Record<string, Route> = {
  cc: {
    name: 'Claude Code',
    tag: 'no key',
    dest: 'Anthropic',
    copy: 'Runs the claude command you already signed in to, on the plan you already pay for. Nothing to paste.',
  },
  anthropic: {
    name: 'Anthropic key',
    tag: 'keychain',
    dest: 'Anthropic',
    copy: 'Your own API key, kept in the macOS keychain and sent only to Anthropic. Web search works on this route.',
  },
  openrouter: {
    name: 'OpenRouter',
    tag: 'curated',
    dest: 'OpenRouter',
    copy: 'Your OpenRouter key and a short list of models, each one checked against the tool-calling tests.',
  },
  local: {
    name: 'Ollama · LM Studio',
    tag: 'experimental',
    dest: '127.0.0.1',
    copy: 'A model server you run yourself. Only localhost or https addresses are accepted, and no key is needed.',
  },
  builtin: {
    name: 'Built-in',
    tag: 'experimental',
    dest: 'no one',
    local: true,
    copy: 'A 1.1 GB model inside the app, on the Mac’s GPU. Nothing leaves this Mac. It picked the right tool 12 times out of 12 in our tests.',
  },
};
const routeCard = $<HTMLElement>('.route');
function pickRoute(key: string) {
  const r = ROUTES[key];
  if (!r) return;
  $('#route-name').textContent = r.name;
  $('#route-tag').textContent = r.tag;
  $('#route-dest').textContent = r.dest;
  $('#route-copy').textContent = r.copy;
  routeCard.dataset.local = String(Boolean(r.local));
}
for (const input of $$<HTMLInputElement>('#routes input')) {
  input.addEventListener('change', () => pickRoute(input.value));
  if (input.checked) pickRoute(input.value);
}

/* ---------- 08 modes: Eclipse recolours the page, the way the app's Ares theme does ---------- */

const root = document.documentElement;
const themeColor = document.querySelector<HTMLMetaElement>('meta[name="theme-color"]');
for (const input of $$<HTMLInputElement>('input[name="mode"]')) {
  input.addEventListener('change', () => {
    const apply = () => {
      if (input.value === 'eclipse') root.dataset.mode = 'eclipse';
      else delete root.dataset.mode;
      $('#hud-mode').textContent = `Mode · ${input.value === 'eclipse' ? 'Eclipse' : 'Normal'}`;
      themeColor?.setAttribute('content', input.value === 'eclipse' ? '#060304' : '#03060c');
    };
    if (reduced.matches || !document.startViewTransition) apply();
    else document.startViewTransition(apply);
  });
}
