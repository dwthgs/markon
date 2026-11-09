import { useSyncExternalStore } from 'react';

const subscribe = (cb: () => void) => {
  window.addEventListener('resize', cb);
  return () => window.removeEventListener('resize', cb);
};
let cached = { width: 0, height: 0 };
const snapshot = () => {
  if (cached.width !== window.innerWidth || cached.height !== window.innerHeight)
    cached = { width: window.innerWidth, height: window.innerHeight };
  return cached;
};

export function useViewport() {
  return useSyncExternalStore(subscribe, snapshot);
}
