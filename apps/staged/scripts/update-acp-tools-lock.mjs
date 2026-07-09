#!/usr/bin/env node
import { execFile } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { promisify } from "node:util";

const appRoot = path.resolve(import.meta.dirname, "..");
const defaultLockFile = path.join(appRoot, "acp-tools.lock.json");

const SUPPORTED_TARGETS = [
  "aarch64-apple-darwin",
  "x86_64-apple-darwin",
  "aarch64-unknown-linux-gnu",
  "x86_64-unknown-linux-gnu",
];

// The Codex ACP executable stays `codex-acp`, but bundled installs must come
// from the maintained Agent Client Protocol package rather than the stale
// Zed package.
const CODEX_ACP_PACKAGE = "@agentclientprotocol/codex-acp";

const TOOL_SPECS = [
  {
    id: "claude-acp",
    binary: "claude-agent-acp",
    package: "@agentclientprotocol/claude-agent-acp",
    dependencyPackage: "@anthropic-ai/claude-agent-sdk",
    nativePackageKey: "claudeAgentSdk",
    includeClaudeCodeVersion: true,
  },
  {
    id: "codex-acp",
    binary: "codex-acp",
    package: CODEX_ACP_PACKAGE,
    dependencyPackage: "@openai/codex",
    nativePackageKey: "openaiCodex",
  },
];

const NPM_TARGET_CONFIG = {
  "aarch64-apple-darwin": {
    npmOs: "darwin",
    npmCpu: "arm64",
    nativePackages: {
      claudeAgentSdk: "@anthropic-ai/claude-agent-sdk-darwin-arm64",
      openaiCodex: "@openai/codex-darwin-arm64",
    },
    nativeExecutables: {
      claudeAgentSdk: "claude",
      openaiCodex: "vendor/aarch64-apple-darwin/bin/codex",
    },
  },
  "x86_64-apple-darwin": {
    npmOs: "darwin",
    npmCpu: "x64",
    nativePackages: {
      claudeAgentSdk: "@anthropic-ai/claude-agent-sdk-darwin-x64",
      openaiCodex: "@openai/codex-darwin-x64",
    },
    nativeExecutables: {
      claudeAgentSdk: "claude",
      openaiCodex: "vendor/x86_64-apple-darwin/bin/codex",
    },
  },
  "aarch64-unknown-linux-gnu": {
    npmOs: "linux",
    npmCpu: "arm64",
    npmLibc: "glibc",
    nativePackages: {
      claudeAgentSdk: "@anthropic-ai/claude-agent-sdk-linux-arm64",
      openaiCodex: "@openai/codex-linux-arm64",
    },
    nativeExecutables: {
      claudeAgentSdk: "claude",
      openaiCodex: "vendor/aarch64-unknown-linux-musl/bin/codex",
    },
  },
  "x86_64-unknown-linux-gnu": {
    npmOs: "linux",
    npmCpu: "x64",
    npmLibc: "glibc",
    nativePackages: {
      claudeAgentSdk: "@anthropic-ai/claude-agent-sdk-linux-x64",
      openaiCodex: "@openai/codex-linux-x64",
    },
    nativeExecutables: {
      claudeAgentSdk: "claude",
      openaiCodex: "vendor/x86_64-unknown-linux-musl/bin/codex",
    },
  },
};

const npmViewCache = new Map();
const execFileAsync = promisify(execFile);

function usage() {
  console.log(`Usage: scripts/update-acp-tools-lock.mjs [--target <triple>]... [--lock-file <path>]

Queries npm for the latest release of each supported ACP bridge tool and
writes acp-tools.lock.json. Fails loudly when a package or one of its
per-target native dependencies cannot be resolved — never silently pins an
older version.

Supported targets:
  ${SUPPORTED_TARGETS.join("\n  ")}

Environment:
  npm registry config     used to resolve packages
  ACP_TOOLS_LOCK_FILE     lockfile path override
`);
}

function parseArgs(argv) {
  const targets = [];
  let lockFile = process.env.ACP_TOOLS_LOCK_FILE ?? defaultLockFile;
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "-h" || arg === "--help") {
      usage();
      process.exit(0);
    }
    if (arg === "--target") {
      const value = argv[++i];
      if (!value) throw new Error("--target requires a value");
      targets.push(value);
      continue;
    }
    if (arg === "--lock-file") {
      const value = argv[++i];
      if (!value) throw new Error("--lock-file requires a value");
      lockFile = path.resolve(value);
      continue;
    }
    throw new Error(`Unknown argument: ${arg}`);
  }

  const selectedTargets = targets.length ? targets : SUPPORTED_TARGETS;
  for (const target of selectedTargets) {
    if (!SUPPORTED_TARGETS.includes(target)) {
      throw new Error(`Unsupported target '${target}'`);
    }
  }
  return { targets: selectedTargets, lockFile };
}

async function npmView(spec, fields) {
  const cacheKey = `${spec}\0${fields.join("\0")}`;
  if (!npmViewCache.has(cacheKey)) {
    npmViewCache.set(
      cacheKey,
      execFileAsync("npm", ["view", spec, ...fields, "--json"], {
        maxBuffer: 10 * 1024 * 1024,
      }).then(({ stdout }) => {
        try {
          return JSON.parse(stdout);
        } catch (error) {
          throw new Error(
            `npm view ${spec} returned invalid JSON: ${
              error instanceof Error ? error.message : String(error)
            }`,
          );
        }
      }),
    );
  }
  return npmViewCache.get(cacheKey);
}

function requireString(value, label) {
  if (typeof value !== "string" || value.trim() === "") {
    throw new Error(`Missing ${label}`);
  }
  return value;
}

function packageDist(metadata, label) {
  const dist = metadata?.dist;
  return {
    tarball: requireString(dist?.tarball, `${label} dist.tarball`),
    integrity: requireString(dist?.integrity, `${label} dist.integrity`),
  };
}

function parseNpmAliasSpec(spec, fallbackPackage) {
  if (!spec.startsWith("npm:")) {
    return { packageName: fallbackPackage, version: spec };
  }
  const aliased = spec.slice("npm:".length);
  const versionSeparator = aliased.lastIndexOf("@");
  if (versionSeparator <= 0) {
    throw new Error(`Unsupported npm alias spec: ${spec}`);
  }
  return {
    packageName: aliased.slice(0, versionSeparator),
    version: aliased.slice(versionSeparator + 1),
  };
}

async function lockToolForTarget(tool, target) {
  const npmTarget = NPM_TARGET_CONFIG[target];
  if (!npmTarget) {
    throw new Error(`No npm target mapping for ${target}`);
  }

  const packageName = tool.package;
  const packageMetadata = await npmView(`${packageName}@latest`, [
    "name",
    "version",
    "dist",
    "dependencies",
    "engines",
    "bin",
  ]);

  if (packageMetadata.name !== packageName) {
    throw new Error(
      `npm package ${packageName} resolved to ${packageMetadata.name}`,
    );
  }

  const version = requireString(
    packageMetadata.version,
    `${packageName} version`,
  );
  const packageInfo = packageDist(packageMetadata, `${packageName}@${version}`);
  const entry = {
    id: tool.id,
    binary: tool.binary,
    source: "npm",
    package: packageName,
    version,
    integrity: packageInfo.integrity,
    tarball: packageInfo.tarball,
    target,
    npmOs: npmTarget.npmOs,
    npmCpu: npmTarget.npmCpu,
    ...(npmTarget.npmLibc ? { npmLibc: npmTarget.npmLibc } : {}),
    nodeEngine: packageMetadata.engines?.node ?? ">=22",
  };

  const dependencyRange = requireString(
    packageMetadata.dependencies?.[tool.dependencyPackage],
    `${packageName} dependency ${tool.dependencyPackage}`,
  );
  const dependencyMetadata = await npmView(
    `${tool.dependencyPackage}@${dependencyRange}`,
    ["name", "version", "dist", "optionalDependencies", "claudeCodeVersion"],
  );
  const dependencyVersion = requireString(
    dependencyMetadata.version,
    `${tool.dependencyPackage}@${dependencyRange} version`,
  );
  const dependencyInfo = packageDist(
    dependencyMetadata,
    `${tool.dependencyPackage}@${dependencyVersion}`,
  );
  entry.dependencyPackage = tool.dependencyPackage;
  entry.dependencyVersion = dependencyVersion;
  entry.dependencyIntegrity = dependencyInfo.integrity;
  entry.dependencyTarball = dependencyInfo.tarball;
  if (tool.includeClaudeCodeVersion) {
    entry.claudeCodeVersion = dependencyMetadata.claudeCodeVersion ?? null;
  }

  const nativePackage = requireString(
    npmTarget.nativePackages?.[tool.nativePackageKey],
    `${target} native package for ${tool.nativePackageKey}`,
  );
  const nativeExecutable = requireString(
    npmTarget.nativeExecutables?.[tool.nativePackageKey],
    `${target} native executable for ${tool.nativePackageKey}`,
  );
  const nativeSpec = requireString(
    dependencyMetadata.optionalDependencies?.[nativePackage],
    `${tool.dependencyPackage}@${dependencyVersion} optional dependency ${nativePackage}`,
  );
  const nativeAlias = parseNpmAliasSpec(nativeSpec, nativePackage);
  const nativeMetadata = await npmView(
    `${nativeAlias.packageName}@${nativeAlias.version}`,
    ["name", "version", "dist"],
  );
  const nativeVersion = requireString(
    nativeMetadata.version,
    `${nativeAlias.packageName}@${nativeAlias.version} version`,
  );
  if (nativeVersion !== nativeAlias.version) {
    throw new Error(
      `${nativeAlias.packageName}@${nativeAlias.version} resolved to ${nativeVersion}`,
    );
  }
  const nativeInfo = packageDist(
    nativeMetadata,
    `${nativeAlias.packageName}@${nativeVersion}`,
  );

  return {
    ...entry,
    nativePackage,
    nativePackageName: nativeMetadata.name ?? nativePackage,
    nativeVersion,
    nativeIntegrity: nativeInfo.integrity,
    nativeTarball: nativeInfo.tarball,
    nativeExecutable,
  };
}

async function main() {
  const { targets, lockFile } = parseArgs(process.argv.slice(2));
  const tools = [];
  for (const tool of TOOL_SPECS) {
    for (const target of targets) {
      tools.push(await lockToolForTarget(tool, target));
    }
  }
  tools.sort((left, right) =>
    `${left.id}:${left.target}`.localeCompare(`${right.id}:${right.target}`),
  );
  await mkdir(path.dirname(lockFile), { recursive: true });
  await writeFile(lockFile, `${JSON.stringify({ tools }, null, 2)}\n`);
  console.log(`Updated ${path.relative(process.cwd(), lockFile)}`);
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : String(error));
  process.exit(1);
});
