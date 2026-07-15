#!/usr/bin/env node
import { execFile } from "node:child_process";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import os from "node:os";
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

// Releases must age past a cooling-off window before they are eligible to
// pin: broken or compromised releases are typically yanked or superseded
// within a day or two of publish, so waiting out the window keeps the daily
// bump from shipping them. When `latest` is still inside the window, the
// newest older stable release that has aged past it is pinned instead.
const DEFAULT_COOLING_OFF_HOURS = 48;

// The Codex ACP executable stays `codex-acp`, but bundled installs must come
// from the maintained Agent Client Protocol package rather than the stale
// Zed package.
const CODEX_ACP_PACKAGE = "@agentclientprotocol/codex-acp";

// `passthroughArgs` is the bridge's CLI-passthrough invocation that prints the
// vendored harness CLI's version. Doctor probes auth (and, for bundled
// installs, freshness) through these same passthrough subcommands
// (`claude-agent-acp --cli …`, `codex-acp cli …`), so a bridge release that
// drops or renames them must fail the smoke check here instead of silently
// breaking every doctor probe in the field.
//
// codex-acp uses `-V` because its entrypoint intercepts a literal `--version`
// anywhere in argv and prints the bridge's own version; only codex's clap
// short flag reaches the vendored binary.
const TOOL_SPECS = [
  {
    id: "claude-acp",
    binary: "claude-agent-acp",
    package: "@agentclientprotocol/claude-agent-acp",
    dependencyPackage: "@anthropic-ai/claude-agent-sdk",
    nativePackageKey: "claudeAgentSdk",
    includeClaudeCodeVersion: true,
    passthroughArgs: ["--cli", "--version"],
  },
  {
    id: "codex-acp",
    binary: "codex-acp",
    package: CODEX_ACP_PACKAGE,
    dependencyPackage: "@openai/codex",
    nativePackageKey: "openaiCodex",
    passthroughArgs: ["cli", "-V"],
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
  console.log(`Usage: scripts/update-acp-tools-lock.mjs [--target <triple>]... [--lock-file <path>] [--skip-smoke] [--cooling-off-hours <hours>]

Queries npm for the newest release of each supported ACP bridge tool that has
aged past the cooling-off window (default ${DEFAULT_COOLING_OFF_HOURS} hours, 0 disables it) and
writes acp-tools.lock.json. Fails loudly when a package or one of its
per-target native dependencies cannot be resolved — never silently pins an
older version than the cooling-off window calls for.

Before writing the lock, each tool's CLI passthrough (the subcommand doctor's
auth/version probes rely on) is smoke-checked against the resolved release by
installing it into a temp prefix and running it on the current platform.
--skip-smoke bypasses this (e.g. on hosts that cannot execute the vendored
binaries).

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
  let skipSmoke = false;
  let coolingOffHours = DEFAULT_COOLING_OFF_HOURS;
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
    if (arg === "--skip-smoke") {
      skipSmoke = true;
      continue;
    }
    if (arg === "--cooling-off-hours") {
      const value = argv[++i];
      if (!value) throw new Error("--cooling-off-hours requires a value");
      coolingOffHours = Number(value);
      if (!Number.isFinite(coolingOffHours) || coolingOffHours < 0) {
        throw new Error("--cooling-off-hours must be a non-negative number");
      }
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
  return { targets: selectedTargets, lockFile, skipSmoke, coolingOffHours };
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

function compareSemver(left, right) {
  const leftCore = left.split("-", 1)[0].split(".").map(Number);
  const rightCore = right.split("-", 1)[0].split(".").map(Number);
  for (let i = 0; i < 3; i += 1) {
    if ((leftCore[i] ?? 0) !== (rightCore[i] ?? 0)) {
      return (leftCore[i] ?? 0) - (rightCore[i] ?? 0);
    }
  }
  // A release outranks any prerelease of the same core version.
  return (left.includes("-") ? 0 : 1) - (right.includes("-") ? 0 : 1);
}

// Publish timestamp of a version from a packument `time` map, or null when
// the registry does not report one (an unknown publish time never counts as
// aged — the cooling-off window cannot be silently waived).
function publishedAtMs(timeMap, version) {
  const published = Date.parse(timeMap?.[version] ?? "");
  return Number.isFinite(published) ? published : null;
}

function hasAged(timeMap, version, coolingOffHours, now) {
  if (coolingOffHours <= 0) return true;
  const published = publishedAtMs(timeMap, version);
  return published !== null && now - published >= coolingOffHours * 3600_000;
}

// Resolve the version to pin for a package: the `latest` dist-tag once it has
// aged past the cooling-off window, otherwise the newest older stable release
// that has. Never resolves past `latest`, so an upstream dist-tag rollback
// (e.g. after a bad release) is honored even when newer versions exist.
async function resolveLatestAgedVersion(packageName, coolingOffHours, now) {
  const packument = await npmView(packageName, [
    "dist-tags",
    "time",
    "versions",
  ]);
  const latest = requireString(
    packument["dist-tags"]?.latest,
    `${packageName} dist-tags.latest`,
  );
  const timeMap = packument.time ?? {};
  if (hasAged(timeMap, latest, coolingOffHours, now)) return latest;
  // npm view returns a bare string instead of a one-element array when the
  // package has a single version.
  const versions = Array.isArray(packument.versions)
    ? packument.versions
    : [packument.versions];
  const candidates = versions.filter(
    (version) =>
      typeof version === "string" &&
      !version.includes("-") &&
      compareSemver(version, latest) < 0 &&
      hasAged(timeMap, version, coolingOffHours, now),
  );
  if (candidates.length === 0) {
    throw new Error(
      `${packageName} has no stable release that has aged past the ` +
        `${coolingOffHours}h cooling-off window (latest ${latest} published ` +
        `${timeMap[latest] ?? "at an unknown time"})`,
    );
  }
  const resolved = candidates.reduce((best, candidate) =>
    compareSemver(candidate, best) > 0 ? candidate : best,
  );
  console.log(
    `${packageName}: latest ${latest} (published ${timeMap[latest]}) is ` +
      `inside the ${coolingOffHours}h cooling-off window; pinning ` +
      `${resolved} instead.`,
  );
  return resolved;
}

const coolingOffLogDedup = new Set();

// npm view returns a single object when a spec matches one version, but an
// array of per-version objects when a range matches several. Pick the highest
// matching version that has aged past the cooling-off window, so a ranged
// dependency still pins the newest eligible release. A bridge release that
// aged past the window had at least one in-range dependency version at
// publish time — necessarily just as aged — so an empty result means the
// range never matched anything, not an over-strict window.
function pickLatestAgedMatch(metadata, label, timeMap, coolingOffHours, now) {
  const matches = Array.isArray(metadata) ? metadata : [metadata];
  if (matches.length === 0) {
    throw new Error(`No versions match ${label}`);
  }
  const pickNewest = (candidates) =>
    candidates.reduce((best, candidate) =>
      compareSemver(
        requireString(candidate?.version, `${label} version`),
        requireString(best?.version, `${label} version`),
      ) > 0
        ? candidate
        : best,
    );
  const aged = matches.filter((candidate) =>
    hasAged(
      timeMap,
      requireString(candidate?.version, `${label} version`),
      coolingOffHours,
      now,
    ),
  );
  if (aged.length === 0) {
    throw new Error(
      `All versions matching ${label} were published within the ` +
        `${coolingOffHours}h cooling-off window`,
    );
  }
  const best = pickNewest(aged);
  const newest = pickNewest(matches);
  // Deduped because this runs once per target with identical inputs.
  if (newest.version !== best.version && !coolingOffLogDedup.has(label)) {
    coolingOffLogDedup.add(label);
    console.log(
      `${label}: newest match ${newest.version} (published ` +
        `${timeMap?.[newest.version] ?? "at an unknown time"}) is inside ` +
        `the ${coolingOffHours}h cooling-off window; pinning ` +
        `${best.version} instead.`,
    );
  }
  return best;
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

async function lockToolForTarget(
  tool,
  target,
  agedVersion,
  coolingOffHours,
  now,
) {
  const npmTarget = NPM_TARGET_CONFIG[target];
  if (!npmTarget) {
    throw new Error(`No npm target mapping for ${target}`);
  }

  const packageName = tool.package;
  const packageMetadata = await npmView(`${packageName}@${agedVersion}`, [
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
  const dependencyMetadata = pickLatestAgedMatch(
    await npmView(`${tool.dependencyPackage}@${dependencyRange}`, [
      "name",
      "version",
      "dist",
      "optionalDependencies",
      "claudeCodeVersion",
    ]),
    `${tool.dependencyPackage}@${dependencyRange}`,
    await npmView(tool.dependencyPackage, ["time"]),
    coolingOffHours,
    now,
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

// Install the resolved release into a temp npm prefix and run the bridge's
// CLI passthrough on the current platform, requiring exit 0 and the locked
// vendored-harness version in the output. Guards the interface doctor's
// probes depend on: the passthrough flags are bridge behavior, not a
// documented stable contract, so a release that breaks them — or one whose
// entrypoint starts answering the probe with the bridge's own version
// instead of the vendored CLI's — must fail here before it gets pinned.
async function smokeCheckPassthrough(tool, locked) {
  const invocation = `${tool.binary} ${tool.passthroughArgs.join(" ")}`;
  const version = locked.version;
  // The version doctor surfaces: the vendored harness CLI's, not the bridge's.
  const harnessVersion = locked.claudeCodeVersion ?? locked.dependencyVersion;
  const prefix = await mkdtemp(path.join(os.tmpdir(), "acp-tools-smoke-"));
  try {
    // npm resolves the bridge's dependency range at install time, so a plain
    // install picks the newest in-range release — not the locked one whenever
    // the cooling-off window pinned older. The override forces the locked
    // resolution (ensure-acp-tools.sh installs the same way), so the check
    // exercises the exact bridge + vendored-harness pair the lock ships.
    // npm silently ignores root overrides when --prefix is passed, so the
    // install must run with the prefix as its working directory instead;
    // writing package.json first makes the prefix the project root, keeping
    // repo config out of the install just as --prefix did.
    await writeFile(
      path.join(prefix, "package.json"),
      `${JSON.stringify({
        name: "acp-tools-smoke",
        private: true,
        overrides: { [locked.dependencyPackage]: locked.dependencyVersion },
      })}\n`,
    );
    await execFileAsync(
      "npm",
      [
        "install",
        "--no-fund",
        "--no-audit",
        "--loglevel=error",
        `${tool.package}@${version}`,
      ],
      { cwd: prefix, maxBuffer: 10 * 1024 * 1024, timeout: 10 * 60 * 1000 },
    );
    const binary = path.join(prefix, "node_modules", ".bin", tool.binary);
    let output;
    try {
      const { stdout, stderr } = await execFileAsync(
        binary,
        tool.passthroughArgs,
        { timeout: 60 * 1000 },
      );
      output = `${stdout}\n${stderr}`.trim();
    } catch (error) {
      throw new Error(
        `Passthrough smoke check failed for ${tool.package}@${version}: ` +
          `\`${invocation}\` did not exit 0 — doctor's auth/version probes ` +
          `would break on this release. ${error instanceof Error ? error.message : String(error)}`,
      );
    }
    if (!output.includes(harnessVersion)) {
      throw new Error(
        `Passthrough smoke check failed for ${tool.package}@${version}: ` +
          `\`${invocation}\` did not print the vendored ` +
          `${tool.dependencyPackage} version ${harnessVersion}: ` +
          `${output || "(empty output)"}. If the output shows the bridge's ` +
          `own version, its entrypoint is intercepting the probe before the ` +
          `passthrough dispatch and doctor's freshness readout would be wrong.`,
      );
    }
    console.log(`Smoke-checked \`${invocation}\`: ${output.split("\n")[0]}`);
  } finally {
    await rm(prefix, { recursive: true, force: true });
  }
}

async function main() {
  const { targets, lockFile, skipSmoke, coolingOffHours } = parseArgs(
    process.argv.slice(2),
  );
  const now = Date.now();
  const tools = [];
  for (const tool of TOOL_SPECS) {
    const agedVersion = await resolveLatestAgedVersion(
      tool.package,
      coolingOffHours,
      now,
    );
    for (const target of targets) {
      tools.push(
        await lockToolForTarget(tool, target, agedVersion, coolingOffHours, now),
      );
    }
  }
  tools.sort((left, right) =>
    `${left.id}:${left.target}`.localeCompare(`${right.id}:${right.target}`),
  );
  if (skipSmoke) {
    console.log("Skipping passthrough smoke checks (--skip-smoke)");
  } else {
    for (const tool of TOOL_SPECS) {
      const locked = tools.find((entry) => entry.id === tool.id);
      if (locked) {
        await smokeCheckPassthrough(tool, locked);
      }
    }
  }
  await mkdir(path.dirname(lockFile), { recursive: true });
  await writeFile(lockFile, `${JSON.stringify({ tools }, null, 2)}\n`);
  console.log(`Updated ${path.relative(process.cwd(), lockFile)}`);
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : String(error));
  process.exit(1);
});
