// Test fixture: packs a minimal `<Name>.app` bundle into a real tar.gz the same
// way the release workflow does (`tar -czf Staged.app.tar.gz Staged.app`).
import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

export function infoPlist(version: string): string {
  return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key>
  <string>com.block.staged</string>
  <key>CFBundleShortVersionString</key>
  <string>${version}</string>
  <key>CFBundleVersion</key>
  <string>${version}</string>
</dict>
</plist>
`;
}

/**
 * Write `archivePath` containing `<appName>/Contents/Info.plist`. Pass `plist`
 * to override the plist contents, or `null` to omit Info.plist entirely.
 */
export function writeAppArchive(
  archivePath: string,
  {
    plist = infoPlist('0.1.9'),
    appName = 'Staged.app',
  }: { plist?: string | null; appName?: string } = {}
) {
  const stageDir = mkdtempSync(join(tmpdir(), 'staged-app-archive-'));
  try {
    const contentsDir = join(stageDir, appName, 'Contents');
    mkdirSync(join(contentsDir, 'MacOS'), { recursive: true });
    writeFileSync(join(contentsDir, 'MacOS', 'staged'), 'binary');
    if (plist !== null) {
      writeFileSync(join(contentsDir, 'Info.plist'), plist);
    }
    execFileSync('tar', ['-czf', archivePath, appName], { cwd: stageDir });
  } finally {
    rmSync(stageDir, { recursive: true, force: true });
  }
}
