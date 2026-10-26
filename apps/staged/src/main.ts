import { mount } from 'svelte';
import './app.css';
import Root from './Root.svelte';

if (
  import.meta.env.PROD &&
  'serviceWorker' in navigator &&
  ['http:', 'https:'].includes(window.location.protocol)
) {
  navigator.serviceWorker
    .register('/sw.js')
    .catch((error) => console.warn('Service worker registration failed', error));
}

// index.html ships #portal-root after #app, but other entry points that mount
// the app (harness fixtures, probes) may not. bits-ui has no fallback for a
// missing portal target — it throws in DEV and silently drops the content in
// PROD — so make sure the element exists as body's last child before mounting.
if (!document.getElementById('portal-root')) {
  const portalRoot = document.createElement('div');
  portalRoot.id = 'portal-root';
  document.body.appendChild(portalRoot);
}

const app = mount(Root, {
  target: document.getElementById('app')!,
});

export default app;
