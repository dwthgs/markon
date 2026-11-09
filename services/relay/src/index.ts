import { type Env, handle } from './handler';

export default {
  fetch(req: Request, env: Env, ctx: { waitUntil(p: Promise<unknown>): void }) {
    return handle(req, env, fetch, ctx);
  },
};
