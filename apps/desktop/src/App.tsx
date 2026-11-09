import { VoiceCore } from '@markon/voice-core';
import { useViewport } from './lib/useViewport';

export function App() {
  const { width, height } = useViewport();
  // HUD centre is 800 px on a large screen; smaller screens shrink it. The core never moves.
  const size = Math.round(Math.min(800, Math.min(width, height) * 0.78));
  return (
    <main className="fixed inset-0 grid place-items-center bg-[radial-gradient(70%_60%_at_50%_45%,#071624_0%,#03060c_70%)]">
      <VoiceCore variant="prime" state="speaking" size={size} demo />
    </main>
  );
}
