import { forwardRef, useState } from 'react';
import { call } from '@/lib/tauri';

/** Type instead of talking. Always available; focused automatically when there is no microphone. */
export const TextInput = forwardRef<HTMLInputElement>(function TextInput(_, ref) {
  const [text, setText] = useState('');
  return (
    <form
      className="absolute bottom-4 left-1/2 w-[min(44ch,40vw)] -translate-x-1/2"
      onSubmit={(e) => {
        e.preventDefault();
        const t = text.trim();
        if (!t) return;
        void call('submit_text', { text: t });
        setText('');
      }}
    >
      <input
        ref={ref}
        aria-label="Message Markon"
        placeholder="Type to Markon…"
        value={text}
        onChange={(e) => setText(e.target.value)}
        className="w-full border border-white/15 bg-transparent px-3 py-1.5 font-mono text-xs text-white/85 outline-none placeholder:text-white/30 focus:border-primary/60"
      />
    </form>
  );
});
