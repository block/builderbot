import { createHash } from 'node:crypto';
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { relative, resolve } from 'node:path';
import { defineConfig, type Plugin, type Rollup } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import tailwindcss from '@tailwindcss/vite';

const port = parseInt(process.env.VITE_PORT || '5174', 10);
const rootDir = import.meta.dirname;
const publicDir = resolve(rootDir, 'public');
const serviceWorkerTemplatePath = resolve(rootDir, 'src/service-worker.js');
const serviceWorkerCacheNamePlaceholder = '__STAGED_CACHE_NAME__';
const serviceWorkerCacheHashLength = 12;
const packageJson = JSON.parse(
  readFileSync(resolve(rootDir, 'package.json'), 'utf8')
) as { version: string };

type HashInput = {
  contents: string | Uint8Array;
  fileName: string;
  kind: 'bundle' | 'public' | 'template';
};

function generatedServiceWorkerPlugin(): Plugin {
  return {
    name: 'staged-generated-service-worker',
    apply: 'build',
    enforce: 'post',
    generateBundle(_options, bundle) {
      const template = readFileSync(serviceWorkerTemplatePath, 'utf8');

      if (!template.includes(serviceWorkerCacheNamePlaceholder)) {
        throw new Error(
          `Service worker template must contain ${serviceWorkerCacheNamePlaceholder}`
        );
      }

      const cacheName = `staged-${hashInputs([
        ...collectBundleInputs(bundle),
        ...collectPublicAssetInputs(publicDir),
        {
          contents: template,
          fileName: 'src/service-worker.js',
          kind: 'template',
        },
      ])}`;

      this.emitFile({
        fileName: 'sw.js',
        source: template.replaceAll(serviceWorkerCacheNamePlaceholder, cacheName),
        type: 'asset',
      });
    },
  };
}

function collectBundleInputs(bundle: Rollup.OutputBundle): HashInput[] {
  return Object.values(bundle).map((output) => ({
    contents: output.type === 'chunk' ? output.code : output.source,
    fileName: output.fileName,
    kind: 'bundle',
  }));
}

function collectPublicAssetInputs(directory: string): HashInput[] {
  if (!existsSync(directory)) {
    return [];
  }

  return collectFiles(directory).map((filePath) => ({
    contents: readFileSync(filePath),
    fileName: toPosixPath(relative(directory, filePath)),
    kind: 'public',
  }));
}

function collectFiles(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const filePath = resolve(directory, entry.name);

    if (entry.isDirectory()) {
      return collectFiles(filePath);
    }

    return entry.isFile() ? [filePath] : [];
  });
}

function hashInputs(inputs: HashInput[]): string {
  const hash = createHash('sha256');

  for (const input of [...inputs].sort(compareHashInputs)) {
    hash.update(input.kind);
    hash.update('\0');
    hash.update(input.fileName);
    hash.update('\0');
    hash.update(
      typeof input.contents === 'string' ? input.contents : Buffer.from(input.contents)
    );
    hash.update('\0');
  }

  return hash.digest('hex').slice(0, serviceWorkerCacheHashLength);
}

function compareHashInputs(left: HashInput, right: HashInput): number {
  return `${left.kind}:${left.fileName}`.localeCompare(`${right.kind}:${right.fileName}`);
}

function toPosixPath(filePath: string): string {
  return filePath.replaceAll('\\', '/');
}

// https://vite.dev/config/
export default defineConfig({
  define: {
    __APP_VERSION__: JSON.stringify(packageJson.version),
  },
  plugins: [svelte(), tailwindcss(), generatedServiceWorkerPlugin()],
  resolve: {
    alias: {
      $lib: resolve(import.meta.dirname, 'src/lib'),
    },
  },
  server: {
    port,
    strictPort: true,
  },
});
