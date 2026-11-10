import { convertFileSrc } from '@tauri-apps/api/core';
import { createContext, useCallback, useContext, useEffect, useRef, useState } from 'react';
import { call, inTauri } from '@/lib/tauri';
import type { Card } from './types';
import { MEDIA_EVENT, picked } from './useCards';

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

/** Markon's pause_media tool: run `act(play)` when it names this card. */
function useMediaControl(title: string, act: (play: boolean) => void) {
  const ref = useRef(act);
  ref.current = act;
  useEffect(() => {
    const on = (e: Event) => {
      const d = (e as CustomEvent<{ which: string; play: boolean }>).detail;
      if (picked(title, d.which)) ref.current(d.play);
    };
    window.addEventListener(MEDIA_EVENT, on);
    return () => window.removeEventListener(MEDIA_EVENT, on);
  }, [title]);
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
  useMediaControl(card.title, (play) => {
    if (play) void ref.current?.play().catch(() => undefined);
    else ref.current?.pause();
  });
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
  // The embed's JS API takes commands over postMessage.
  const send = useCallback(
    (func: string, args: unknown[] = []) =>
      ref.current?.contentWindow?.postMessage(
        JSON.stringify({ event: 'command', func, args }),
        'https://www.youtube-nocookie.com',
      ),
    [],
  );
  useEffect(() => {
    send('setVolume', [ducked ? DUCKED * 100 : 100]);
  }, [ducked, send]);
  useMediaControl(card.title, (play) => send(play ? 'playVideo' : 'pauseVideo'));
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
    <div className="relative flex h-full items-center gap-3 overflow-hidden">
      {np.artwork ? (
        <img
          src={np.artwork}
          alt=""
          aria-hidden="true"
          className="pointer-events-none absolute inset-0 h-full w-full scale-125 object-cover opacity-20 blur-2xl"
        />
      ) : null}
      {np.artwork ? (
        <img
          src={np.artwork}
          alt=""
          className="relative aspect-square h-full max-h-24 shrink-0 object-cover shadow-[0_0_18px_rgba(0,0,0,.6)]"
          draggable={false}
        />
      ) : null}
      <div className="relative flex min-w-0 flex-1 flex-col gap-1.5">
        <div className="min-w-0">
          <p className="truncate text-[14px] text-white">{np.track || 'Nothing playing'}</p>
          <p className="truncate text-white/55">{np.artist}</p>
        </div>
        <div className="h-0.5 bg-white/10" aria-hidden="true">
          <div className="h-full bg-primary shadow-[0_0_6px_var(--primary)]" style={{ width: `${pct}%` }} />
        </div>
        <div className="flex items-center justify-between">
          <div className="-ml-1.5 flex items-center">
            <IconButton label="Previous track" onClick={() => act('previous')} d={PREV} />
            <IconButton
              label={playing ? 'Pause' : 'Play'}
              onClick={() => act(playing ? 'pause' : 'play')}
              d={playing ? PAUSE : PLAY}
              big
            />
            <IconButton label="Next track" onClick={() => act('next')} d={NEXT} />
          </div>
          <span className="text-[10px] tabular-nums text-white/40">
            {clock(np.position_s)} / {clock(np.duration_s)}
          </span>
        </div>
      </div>
    </div>
  );
}

const PLAY = 'M8 5v14l11-7z';
const PAUSE = 'M7 5h3.5v14H7zM13.5 5H17v14h-3.5z';
const PREV = 'M6 6h2v12H6zM9.5 12 18 6v12z';
const NEXT = 'M16 6h2v12h-2zM14.5 12 6 18V6z';

function IconButton({
  label,
  d,
  onClick,
  big,
}: {
  label: string;
  d: string;
  onClick: () => void;
  big?: boolean;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      onClick={onClick}
      className="grid place-items-center rounded-full p-1.5 text-white/70 transition-colors hover:text-white focus-visible:text-white focus-visible:outline focus-visible:outline-primary"
    >
      <svg
        viewBox="0 0 24 24"
        aria-hidden="true"
        className={big ? 'size-5 text-white' : 'size-4'}
        fill="currentColor"
      >
        <path d={d} />
      </svg>
    </button>
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
