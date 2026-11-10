import { convertFileSrc } from '@tauri-apps/api/core';
import { createContext, useContext, useEffect, useRef, useState } from 'react';
import { call, inTauri } from '@/lib/tauri';
import type { Card } from './types';

/** True while Markon listens, thinks or speaks: media plays quietly underneath. */
export const DuckContext = createContext(false);
const DUCKED = 0.2;

/** A local file the core allowed for this card (the asset protocol's scope starts empty). */
function fileSrc(path: string) {
  try {
    return convertFileSrc(path);
  } catch {
    return `asset://localhost/${encodeURIComponent(path)}`;
  }
}

function Credit({ children }: { children: React.ReactNode }) {
  return <p className="mt-1 truncate text-[10px] text-white/45">{children}</p>;
}

function ImageBody({ card }: { card: Card }) {
  const b = card.body ?? {};
  return (
    <figure className="flex h-full flex-col">
      <img
        src={fileSrc(b.path)}
        alt={card.title}
        className="min-h-0 w-full flex-1 bg-black/40 object-contain"
        draggable={false}
      />
      {b.credit ? <Credit>{b.credit}</Credit> : null}
    </figure>
  );
}

function Player({ card }: { card: Card }) {
  const b = card.body ?? {};
  const ducked = useContext(DuckContext);
  const ref = useRef<HTMLMediaElement | null>(null);
  const keep = (el: HTMLMediaElement | null) => {
    ref.current = el;
  };
  useEffect(() => {
    if (ref.current) ref.current.volume = ducked ? DUCKED : 1;
  }, [ducked]);
  const src = b.path ? fileSrc(b.path) : b.url;
  const props = {
    src,
    controls: true,
    autoPlay: true,
    className: 'block h-full w-full bg-black object-contain',
  };
  return card.kind === 'audio' ? (
    <div className="flex h-full flex-col justify-center">
      <audio ref={keep} {...props} className="w-full" />
    </div>
  ) : (
    <video ref={keep} {...props} playsInline />
  );
}

function YouTube({ card }: { card: Card }) {
  const id: string = card.body.youtube;
  const ducked = useContext(DuckContext);
  const ref = useRef<HTMLIFrameElement>(null);
  useEffect(() => {
    // The embed's JS API takes commands over postMessage.
    ref.current?.contentWindow?.postMessage(
      JSON.stringify({ event: 'command', func: 'setVolume', args: [ducked ? DUCKED * 100 : 100] }),
      'https://www.youtube-nocookie.com',
    );
  }, [ducked]);
  return (
    <iframe
      ref={ref}
      title={card.title}
      src={`https://www.youtube-nocookie.com/embed/${id}?autoplay=1&enablejsapi=1&playsinline=1&rel=0`}
      allow="autoplay; encrypted-media; picture-in-picture"
      allowFullScreen
      referrerPolicy="strict-origin-when-cross-origin"
      className="block h-full w-full border-0 bg-black"
    />
  );
}

type NowPlaying = {
  track: string;
  artist: string;
  album: string;
  artwork: string;
  state: string;
  position_s: number;
  duration_s: number;
};

const clock = (s: number) => `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, '0')}`;

function Spotify({ card }: { card: Card }) {
  const [np, setNp] = useState<NowPlaying>(card.body);
  useEffect(() => setNp(card.body), [card.body]);
  useEffect(() => {
    if (!inTauri()) return;
    const poll = async () => {
      const n = await call<NowPlaying>('spotify_control', { action: 'status' }).catch(() => undefined);
      if (n) setNp(n);
    };
    const t = setInterval(poll, 2000);
    return () => clearInterval(t);
  }, []);
  const act = async (action: string) => {
    const n = await call<NowPlaying>('spotify_control', { action }).catch(() => undefined);
    if (n) setNp(n);
  };
  const playing = np.state === 'playing';
  const pct = np.duration_s ? Math.min(100, (np.position_s / np.duration_s) * 100) : 0;
  return (
    <div className="flex h-full gap-3">
      {np.artwork ? (
        <img
          src={np.artwork}
          alt=""
          className="aspect-square h-full max-h-40 object-cover"
          draggable={false}
        />
      ) : null}
      <div className="flex min-w-0 flex-1 flex-col justify-center gap-1">
        <p className="truncate text-[13px] text-white">{np.track || 'Nothing playing'}</p>
        <p className="truncate text-white/60">{np.artist}</p>
        <p className="truncate text-[10px] text-white/40">{np.album}</p>
        <div className="mt-1 h-1 bg-white/10" aria-hidden="true">
          <div className="h-full bg-primary" style={{ width: `${pct}%` }} />
        </div>
        <p className="text-[10px] text-white/40">
          {clock(np.position_s)} / {clock(np.duration_s)}
        </p>
        <div className="mt-1 flex gap-1.5">
          <button
            type="button"
            className="hud-btn py-0.5"
            aria-label="Previous track"
            onClick={() => act('previous')}
          >
            ⏮
          </button>
          <button
            type="button"
            className="hud-btn py-0.5"
            aria-label={playing ? 'Pause' : 'Play'}
            onClick={() => act(playing ? 'pause' : 'play')}
          >
            {playing ? '⏸' : '▶'}
          </button>
          <button
            type="button"
            className="hud-btn py-0.5"
            aria-label="Next track"
            onClick={() => act('next')}
          >
            ⏭
          </button>
        </div>
      </div>
    </div>
  );
}

export function MediaBody({ card }: { card: Card }) {
  const b = card.body ?? {};
  if (card.status !== 'done') return <p className="whitespace-pre-wrap">{b.reason ?? 'Loading…'}</p>;
  switch (card.kind) {
    case 'image':
      return <ImageBody card={card} />;
    case 'music':
      return <Spotify card={card} />;
    default:
      return b.youtube ? <YouTube card={card} /> : <Player card={card} />;
  }
}
