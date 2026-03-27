import { isDesktopApp } from '../api';

// E-PENPAL-EXTERNAL-LINKS: opens paths in new Tauri WebviewWindow for desktop app.
export async function openInNewWindow(path: string, title: string): Promise<boolean> {
  if (!isDesktopApp) return false;
  const { WebviewWindow } = await import('@tauri-apps/api/webviewWindow');
  const label = `win-${Date.now()}`;
  new WebviewWindow(label, {
    url: path,
    title,
    width: 1200,
    height: 800,
  });
  return true;
}
