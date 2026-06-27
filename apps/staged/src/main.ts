import { mount } from 'svelte';
import './app.css';
import App from './App.svelte';

if (
  import.meta.env.PROD &&
  'serviceWorker' in navigator &&
  ['http:', 'https:'].includes(window.location.protocol)
) {
  navigator.serviceWorker
    .register('/sw.js')
    .catch((error) => console.warn('Service worker registration failed', error));
}

const app = mount(App, {
  target: document.getElementById('app')!,
});

export default app;
