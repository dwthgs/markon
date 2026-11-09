import { type RefObject, useEffect, useImperativeHandle, useRef } from 'react';
import { drawVariant, type Variant } from './draw';
import { CoreEngine, type CoreState, type Features } from './engine';

/** Push live audio features straight into the running engine, bypassing React renders. */
export interface FeatureFeed {
  push(f: Features): void;
}

export interface VoiceCoreProps {
  variant?: Variant;
  state: CoreState;
  /** Latest features via props. For 30 Hz streams prefer `feedRef`, which does not re-render. */
  features?: Features;
  /** CSS pixel size of the square canvas. */
  size: number;
  feedRef?: RefObject<FeatureFeed | null>;
  /** Animate with the built-in fake speech when no live features arrive. */
  demo?: boolean;
  seed?: number;
  reducedMotion?: boolean;
  className?: string;
}

export function VoiceCore({
  variant = 'prime',
  state,
  features,
  size,
  feedRef,
  demo = false,
  seed = 1,
  reducedMotion,
  className,
}: VoiceCoreProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const engineRef = useRef<CoreEngine | null>(null);
  if (engineRef.current === null) engineRef.current = new CoreEngine({ seed, demo });
  const engine = engineRef.current;
  const variantRef = useRef(variant);
  variantRef.current = variant;

  useImperativeHandle(feedRef, () => ({ push: (f) => engine.pushFeatures(f) }), [engine]);

  useEffect(() => engine.setState(state), [engine, state]);
  useEffect(() => {
    engine.demo = demo;
  }, [engine, demo]);
  useEffect(() => {
    if (features) engine.pushFeatures(features);
  }, [engine, features]);
  useEffect(() => {
    const mq = window.matchMedia?.('(prefers-reduced-motion: reduce)');
    const apply = () => {
      engine.reducedMotion = reducedMotion ?? mq?.matches ?? false;
    };
    apply();
    mq?.addEventListener('change', apply);
    return () => mq?.removeEventListener('change', apply);
  }, [engine, reducedMotion]);

  useEffect(() => {
    const cv = canvasRef.current;
    const c = cv?.getContext('2d');
    if (!cv || !c) return;
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const W = Math.round(size * dpr);
    cv.width = W;
    cv.height = W;
    let last = performance.now();
    let raf = requestAnimationFrame(function frame(now) {
      engine.advance((now - last) / 1000);
      last = now;
      drawVariant(engine, c, W, variantRef.current);
      raf = requestAnimationFrame(frame);
    });
    return () => cancelAnimationFrame(raf);
  }, [engine, size]);

  return (
    <canvas
      ref={canvasRef}
      className={className}
      style={{ width: size, height: size, display: 'block' }}
      role="img"
      aria-label={`Voice core, ${state}`}
    />
  );
}
