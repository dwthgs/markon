/** The subset of Cloudflare KV the relay uses. */
export interface KV {
  get(key: string): Promise<string | null>;
  put(key: string, value: string, opts?: { expirationTtl?: number }): Promise<void>;
}

/** In-memory KV for tests and local runs. */
export class MemoryKV implements KV {
  private m = new Map<string, string>();
  async get(key: string) {
    return this.m.get(key) ?? null;
  }
  async put(key: string, value: string) {
    this.m.set(key, value);
  }
  dump() {
    return Object.fromEntries(this.m);
  }
}
