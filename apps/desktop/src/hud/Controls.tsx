/** Top-right controls. */
export function Controls(p: {
  onHistory: () => void;
  onTidy: () => void;
  onClear: () => void;
  onCamera: () => void;
}) {
  return (
    <nav aria-label="Canvas controls" className="absolute top-3 right-4 flex gap-1.5">
      <button type="button" className="hud-btn py-1 text-[10px]" onClick={p.onHistory}>
        History
      </button>
      <button type="button" className="hud-btn py-1 text-[10px]" onClick={p.onTidy}>
        Tidy
      </button>
      <button type="button" className="hud-btn py-1 text-[10px]" onClick={p.onClear}>
        Clear view
      </button>
      <button type="button" className="hud-btn py-1 text-[10px]" onClick={p.onCamera}>
        Camera
      </button>
    </nav>
  );
}
