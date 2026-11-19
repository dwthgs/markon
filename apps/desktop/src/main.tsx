import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import { startFpsProbe } from './lib/fpsProbe';
import './components/thegridcn-theme.css';
import './components/thegridcn-theme-ares.css';
import './styles.css';

if (import.meta.env.DEV && new URLSearchParams(location.search).has('e2e')) {
  (await import('./e2e/fakeTauri')).installFakeTauri();
}

createRoot(document.getElementById('root') as HTMLElement).render(
  <StrictMode>
    <App />
  </StrictMode>,
);

if (import.meta.env.DEV) startFpsProbe();
