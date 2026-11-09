export type CardKind = 'agent' | 'file' | 'page' | 'app' | 'result' | 'note';
export type CardStatus = 'running' | 'done' | 'error' | 'blocked';

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
