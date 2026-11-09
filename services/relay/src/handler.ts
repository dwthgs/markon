/*
 * Markon free-trial relay: device registers → short-lived JWT → /v1/messages proxied to
 * Anthropic with our key (streaming kept) → per-device daily quota in KV → token counts logged per
 * device, never message content. Abuse rules live in KV under "config", so they change without a deploy.
 */
import { sign, verify } from './jwt';
import type { KV } from './kv';

export interface Env {
  ANTHROPIC_API_KEY: string;
  JWT_SECRET: string;
  KV: KV;
  ANTHROPIC_URL?: string;
}

interface Ctx {
  waitUntil(p: Promise<unknown>): void;
}

interface Config {
  daily_messages: number;
  rate_per_minute: number;
  blocklist: string[];
  max_tokens: number;
  models: string[];
}

const DEFAULTS: Config = {
  daily_messages: 30,
  rate_per_minute: 6,
  blocklist: [],
  max_tokens: 8192,
  models: ['claude-opus-5-5', 'claude-sonnet-5-5', 'claude-haiku-5-5'],
};
const TOKEN_TTL = 3600;
const DAY = 86_400;
/** Usage records are deleted 90 days after last use. */
const USAGE_TTL = 90 * DAY;

const json = (status: number, body: unknown, headers: Record<string, string> = {}) =>
  new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json', ...headers } });
const fail = (status: number, code: string, message: string, headers: Record<string, string> = {}) =>
  json(status, { type: 'error', error: { type: code, code, message } }, headers);

async function config(kv: KV): Promise<Config> {
  try {
    return { ...DEFAULTS, ...JSON.parse((await kv.get('config')) ?? '{}') };
  } catch {
    return DEFAULTS;
  }
}

const today = () => new Date().toISOString().slice(0, 10);
const nowSec = () => Math.floor(Date.now() / 1000);
const validDevice = (d: unknown): d is string => typeof d === 'string' && /^[A-Za-z0-9-]{8,64}$/.test(d);

async function device(req: Request, env: Env): Promise<string | null> {
  const auth = req.headers.get('authorization') ?? '';
  const token = auth.startsWith('Bearer ') ? auth.slice(7) : '';
  return (await verify(token, env.JWT_SECRET, nowSec()))?.sub ?? null;
}

async function bump(kv: KV, key: string, ttl: number): Promise<number> {
  // shortcut: KV is eventually consistent, so counts can run slightly over under bursts; move to a
  // Durable Object if the trial is abused.
  const n = Number((await kv.get(key)) ?? '0') + 1;
  await kv.put(key, String(n), { expirationTtl: ttl });
  return n;
}

/** Read token counts out of the SSE stream as it passes. Content is never stored. */
function countTokens(
  stream: ReadableStream<Uint8Array>,
  done: (input: number, output: number) => Promise<void>,
) {
  const dec = new TextDecoder();
  let buf = '';
  let input = 0;
  let output = 0;
  return stream.pipeThrough(
    new TransformStream<Uint8Array, Uint8Array>({
      transform(chunk, c) {
        c.enqueue(chunk);
        buf += dec.decode(chunk, { stream: true });
        let i = buf.indexOf('\n\n');
        while (i >= 0) {
          const ev = buf.slice(0, i);
          buf = buf.slice(i + 2);
          for (const line of ev.split('\n')) {
            if (!line.startsWith('data:')) continue;
            try {
              const d = JSON.parse(line.slice(5));
              if (d.type === 'message_start') input = d.message?.usage?.input_tokens ?? input;
              if (d.type === 'message_delta') output = d.usage?.output_tokens ?? output;
            } catch {
              /* not JSON: ignore */
            }
          }
          i = buf.indexOf('\n\n');
        }
      },
      async flush() {
        await done(input, output);
      },
    }),
  );
}

export async function handle(req: Request, env: Env, upstream: typeof fetch, ctx?: Ctx): Promise<Response> {
  const url = new URL(req.url);
  const cfg = await config(env.KV);

  if (req.method === 'POST' && url.pathname === '/v1/register') {
    const body = (await req.json().catch(() => ({}))) as { device_id?: unknown };
    if (!validDevice(body.device_id))
      return fail(400, 'bad_device', 'device_id must be 8-64 letters, digits or dashes');
    if (cfg.blocklist.includes(body.device_id))
      return fail(403, 'blocked', 'This device cannot use the free trial.');
    const token = await sign({ sub: body.device_id, exp: nowSec() + TOKEN_TTL }, env.JWT_SECRET);
    return json(200, { token, expires_in: TOKEN_TTL });
  }

  const dev = await device(req, env);
  if (!dev) return fail(401, 'unauthorized', 'Register this device again.');
  if (cfg.blocklist.includes(dev)) return fail(403, 'blocked', 'This device cannot use the free trial.');
  const quotaKey = `quota:${dev}:${today()}`;

  if (req.method === 'GET' && url.pathname === '/v1/quota') {
    return json(200, {
      used: Number((await env.KV.get(quotaKey)) ?? '0'),
      limit: cfg.daily_messages,
      day: today(),
    });
  }

  if (req.method === 'POST' && url.pathname === '/v1/messages') {
    const body = (await req.json().catch(() => null)) as { model?: string; max_tokens?: number } | null;
    if (!body || typeof body.model !== 'string') return fail(400, 'bad_request', 'Invalid request body.');
    if (!cfg.models.includes(body.model))
      return fail(400, 'model_not_allowed', `The trial does not offer ${body.model}.`);
    const minute = Math.floor(Date.now() / 60_000);
    if ((await bump(env.KV, `rate:${dev}:${minute}`, 120)) > cfg.rate_per_minute)
      return fail(429, 'rate_limited', 'Slow down a little.', { 'retry-after': '60' });
    const used = Number((await env.KV.get(quotaKey)) ?? '0');
    if (used >= cfg.daily_messages)
      return fail(
        429,
        'quota_exhausted',
        'The free trial is used up for today. Add your own Anthropic or OpenRouter key to keep going.',
      );
    await env.KV.put(quotaKey, String(used + 1), { expirationTtl: 2 * DAY });
    body.max_tokens = Math.min(body.max_tokens ?? cfg.max_tokens, cfg.max_tokens);

    const res = await upstream(`${env.ANTHROPIC_URL ?? 'https://api.anthropic.com'}/v1/messages`, {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
        'x-api-key': env.ANTHROPIC_API_KEY,
        'anthropic-version': req.headers.get('anthropic-version') ?? '2026-10-21',
      },
      body: JSON.stringify(body),
    });
    if (!res.ok || !res.body) {
      console.error(`[relay] upstream ${res.status} for device ${dev.slice(0, 8)}`);
      return new Response(res.body, { status: res.status, headers: { 'content-type': 'application/json' } });
    }
    const usageKey = `usage:${dev}:${today()}`;
    let record: Promise<unknown> = Promise.resolve();
    const counted = countTokens(res.body, async (input, output) => {
      record = (async () => {
        const prev = JSON.parse((await env.KV.get(usageKey)) ?? '{"requests":0,"input":0,"output":0}');
        await env.KV.put(
          usageKey,
          JSON.stringify({
            requests: prev.requests + 1,
            input: prev.input + input,
            output: prev.output + output,
            at: Date.now(),
          }),
          { expirationTtl: USAGE_TTL },
        );
      })();
      ctx?.waitUntil(record);
      await record;
    });
    return new Response(counted, {
      status: 200,
      headers: { 'content-type': res.headers.get('content-type') ?? 'text/event-stream' },
    });
  }

  return fail(404, 'not_found', 'Unknown endpoint.');
}
