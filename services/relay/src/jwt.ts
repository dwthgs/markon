/** Minimal HS256 JWTs with WebCrypto (available in Workers and Node). */
const enc = new TextEncoder();
const b64url = (b: ArrayBuffer | Uint8Array) =>
  btoa(String.fromCharCode(...new Uint8Array(b)))
    .replaceAll('+', '-')
    .replaceAll('/', '_')
    .replace(/=+$/, '');
const fromB64url = (s: string) =>
  Uint8Array.from(atob(s.replaceAll('-', '+').replaceAll('_', '/')), (c) => c.charCodeAt(0));

const key = (secret: string) =>
  crypto.subtle.importKey('raw', enc.encode(secret), { name: 'HMAC', hash: 'SHA-256' }, false, [
    'sign',
    'verify',
  ]);

export interface Claims {
  sub: string;
  exp: number;
}

export async function sign(claims: Claims, secret: string) {
  const head = b64url(enc.encode(JSON.stringify({ alg: 'HS256', typ: 'JWT' })));
  const body = b64url(enc.encode(JSON.stringify(claims)));
  const sig = await crypto.subtle.sign('HMAC', await key(secret), enc.encode(`${head}.${body}`));
  return `${head}.${body}.${b64url(sig)}`;
}

/** The claims if the signature is valid and the token has not expired, else null. */
export async function verify(token: string, secret: string, nowSec: number): Promise<Claims | null> {
  const [head, body, sig] = token.split('.');
  if (!head || !body || !sig) return null;
  try {
    const ok = await crypto.subtle.verify(
      'HMAC',
      await key(secret),
      fromB64url(sig),
      enc.encode(`${head}.${body}`),
    );
    if (!ok) return null;
    const claims = JSON.parse(new TextDecoder().decode(fromB64url(body))) as Claims;
    return typeof claims.sub === 'string' && claims.exp > nowSec ? claims : null;
  } catch {
    return null;
  }
}
