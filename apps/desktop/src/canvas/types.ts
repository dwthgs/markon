export type CardKind =
  | 'agent'
  | 'file'
  | 'page'
  | 'app'
  | 'result'
  | 'note'
  | 'image'
  | 'audio'
  | 'video'
  | 'music';

/** Cards that show a picture or play something: they get two slots of height. */
export const MEDIA: ReadonlySet<CardKind> = new Set(['image', 'audio', 'video', 'music']);
/** `closed` is not shown: it is Markon asking the screen to close cards (`body.which`). */
export type CardStatus = 'running' | 'done' | 'error' | 'blocked' | 'closed';

/** Mirrors `markon_tools::cards::Card`. */
export interface Card {
  id: string;
  turn_id: number;
  kind: CardKind;
  title: string;
  status: CardStatus;
  // biome-ignore lint/suspicious/noExplicitAny: card bodies are free-form JSON from the core, read defensively.
  body: any;
  updated_ms: number;
}
