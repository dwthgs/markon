/** A fake 2D context that records every call and property write, so two runs can be compared. */
export function recordingContext() {
  const log: string[] = [];
  const round = (v: unknown) => (typeof v === 'number' ? v.toFixed(4) : String(v));
  const gradient = { addColorStop: (o: number, c: string) => log.push(`stop ${round(o)} ${c}`) };
  const target: Record<string, unknown> = {};
  const ctx = new Proxy(target, {
    get(_t, key: string) {
      if (key in target) return target[key];
      return (...args: unknown[]) => {
        log.push(`${key}(${args.map(round).join(',')})`);
        return key.startsWith('create') ? gradient : undefined;
      };
    },
    set(_t, key: string, value) {
      log.push(`${key}=${round(value)}`);
      target[key] = value;
      return true;
    },
  }) as unknown as CanvasRenderingContext2D;
  return {
    ctx,
    digest: () => {
      let h = 2166136261;
      for (const line of log)
        for (let i = 0; i < line.length; i++) h = Math.imul(h ^ line.charCodeAt(i), 16777619);
      return `${log.length}:${(h >>> 0).toString(16)}`;
    },
  };
}
