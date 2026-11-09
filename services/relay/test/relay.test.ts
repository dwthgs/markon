import { beforeEach, describe, expect, it, vi } from 'vitest';
import { type Env, handle } from '../src/handler';
import { MemoryKV } from '../src/kv';

const KEY = 'sk-ant-relay-secret-not-real';
const SSE = [
  'event: message_start\ndata: {"type":"message_start","message":{"usage":{"input_tokens":12}}}\n\n',
  'event: content_block_delta\ndata: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"private words"}}\n\n',
  'event: message_delta\ndata: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}\n\n',
  'event: message_stop\ndata: {"type":"message_stop"}\n\n',
];

let env: Env;
let upstream: ReturnType<typeof vi.fn>;
let waits: Promise<unknown>[];
const ctx = { waitUntil: (p: Promise<unknown>) => waits.push(p) };

beforeEach(() => {
  env = { ANTHROPIC_API_KEY: KEY, JWT_SECRET: 'test-jwt-secret-32-bytes-long!!!', KV: new MemoryKV() };
  waits = [];
  upstream = vi.fn(async (_url: string, init: RequestInit) => {
    expect(new Headers(init.headers).get('x-api-key')).toBe(KEY);
    const enc = new TextEncoder();
    const body = new ReadableStream({
      start(c) {
        for (const e of SSE) c.enqueue(enc.encode(e));
        c.close();
      },
    });
    return new Response(body, { headers: { 'content-type': 'text/event-stream' } });
  });
});

const call = (path: string, init: RequestInit = {}) =>
  handle(new Request(`https://relay.test${path}`, init), env, upstream as unknown as typeof fetch, ctx);
const register = async (device = 'device-abc-123') => {
  const r = await call('/v1/register', { method: 'POST', body: JSON.stringify({ device_id: device }) });
  return (await r.json()) as { token: string; expires_in: number };
};
const message = (token: string, body: object = {}) =>
  call('/v1/messages', {
    method: 'POST',
    headers: { authorization: `Bearer ${token}`, 'anthropic-version': '2026-10-21' },
    body: JSON.stringify({
      model: 'claude-opus-5-5',
      max_tokens: 100000,
      stream: true,
      messages: [{ role: 'user', content: 'hi' }],
      ...body,
    }),
  });

describe('relay', () => {
  it('registers a device and returns a short-lived token', async () => {
    const t = await register();
    expect(t.token.split('.')).toHaveLength(3);
    expect(t.expires_in).toBeLessThanOrEqual(3600);
  });

  it('refuses bad device ids', async () => {
    const r = await call('/v1/register', { method: 'POST', body: JSON.stringify({ device_id: '../etc' }) });
    expect(r.status).toBe(400);
  });

  it('proxies /v1/messages with our key and preserves streaming', async () => {
    const { token } = await register();
    const r = await message(token);
    expect(r.status).toBe(200);
    expect(r.headers.get('content-type')).toContain('text/event-stream');
    const text = await r.text();
    expect(text).toContain('private words');
    expect(text).not.toContain(KEY);
    const sent = JSON.parse(upstream.mock.calls[0]?.[1].body as string);
    expect(sent.max_tokens).toBeLessThanOrEqual(8192);
  });

  it('logs token counts per device but never message content', async () => {
    const { token } = await register();
    await (await message(token)).text();
    await Promise.all(waits);
    const all = (env.KV as MemoryKV).dump();
    const usageKey = Object.keys(all).find((k) => k.startsWith('usage:device-abc-123:'));
    expect(JSON.parse(all[usageKey ?? ''] ?? '{}')).toMatchObject({ requests: 1, input: 12, output: 5 });
    const dump = JSON.stringify(all);
    expect(dump).not.toContain('private words');
    expect(dump).not.toContain('hi"');
  });

  it('enforces the daily quota with a machine-readable code', async () => {
    await env.KV.put('config', JSON.stringify({ daily_messages: 2 }));
    const { token } = await register();
    expect((await message(token)).status).toBe(200);
    expect((await message(token)).status).toBe(200);
    const r = await message(token);
    expect(r.status).toBe(429);
    const body = (await r.json()) as { error: { code: string; type: string } };
    expect(body.error.code).toBe('quota_exhausted');
    const q = await call('/v1/quota', { headers: { authorization: `Bearer ${token}` } });
    expect(await q.json()).toMatchObject({ used: 2, limit: 2 });
  });

  it('applies abuse rules from KV without a deploy', async () => {
    const { token } = await register('device-blocked-1');
    await env.KV.put('config', JSON.stringify({ blocklist: ['device-blocked-1'] }));
    const r = await message(token);
    expect(r.status).toBe(403);
    expect(((await r.json()) as { error: { code: string } }).error.code).toBe('blocked');

    await env.KV.put('config', JSON.stringify({ rate_per_minute: 1 }));
    const t2 = (await register('device-fast-1')).token;
    expect((await message(t2)).status).toBe(200);
    const limited = await message(t2);
    expect(limited.status).toBe(429);
    expect(((await limited.json()) as { error: { code: string } }).error.code).toBe('rate_limited');
  });

  it('rejects missing, forged and expired tokens', async () => {
    expect((await message('nope')).status).toBe(401);
    const { token } = await register();
    const [h, p] = token.split('.');
    expect((await message(`${h}.${p}.forged`)).status).toBe(401);
    vi.useFakeTimers();
    vi.setSystemTime(Date.now() + 2 * 3600_000);
    expect((await message(token)).status).toBe(401);
    vi.useRealTimers();
  });

  it('only allows the trial models', async () => {
    const { token } = await register();
    expect((await message(token, { model: 'claude-fable-5-1' })).status).toBe(400);
  });

  it('never logs the key', async () => {
    const spy = vi.spyOn(console, 'log');
    const err = vi.spyOn(console, 'error');
    upstream.mockImplementationOnce(
      async () => new Response('{"error":{"type":"overloaded_error"}}', { status: 529 }),
    );
    const { token } = await register();
    expect((await message(token)).status).toBe(529);
    for (const c of [...spy.mock.calls, ...err.mock.calls]) expect(JSON.stringify(c)).not.toContain(KEY);
  });

  it('adds well under 120 ms before the first upstream byte', async () => {
    const { token } = await register();
    await message(token); // warm up
    const t0 = performance.now();
    let atUpstream = 0;
    upstream.mockImplementationOnce(async () => {
      atUpstream = performance.now();
      return new Response('data: {}\n\n', { headers: { 'content-type': 'text/event-stream' } });
    });
    await message(token);
    const overhead = atUpstream - t0;
    console.info(`relay overhead before upstream: ${overhead.toFixed(2)} ms (in-memory KV)`);
    expect(overhead).toBeLessThan(120);
  });
});
