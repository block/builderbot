import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';

const version = process.argv[2];

if (!version) {
  console.error('Usage: node scripts/bump-version.mjs <version>');
  process.exit(1);
}

if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(version)) {
  console.error(`Invalid version "${version}"`);
  process.exit(1);
}

const packageJsonPath = resolve(process.cwd(), 'package.json');
const tauriConfigPath = resolve(process.cwd(), 'src-tauri/tauri.conf.json');
const cargoTomlPath = resolve(process.cwd(), 'src-tauri/Cargo.toml');

const packageJson = JSON.parse(readFileSync(packageJsonPath, 'utf8'));
if (packageJson.version !== version) {
  packageJson.version = version;
  writeFileSync(packageJsonPath, `${JSON.stringify(packageJson, null, 2)}\n`);
}

const tauriConfig = JSON.parse(readFileSync(tauriConfigPath, 'utf8'));
if (tauriConfig.version !== version) {
  tauriConfig.version = version;
  writeFileSync(tauriConfigPath, `${JSON.stringify(tauriConfig, null, 2)}\n`);
}

const cargoToml = readFileSync(cargoTomlPath, 'utf8');
const currentCargoVersion = cargoToml.match(/^version = "(.*)"$/m)?.[1];
if (!currentCargoVersion) {
  throw new Error(`Could not update version in ${cargoTomlPath}`);
}
const updatedCargoToml =
  currentCargoVersion === version
    ? cargoToml
    : cargoToml.replace(/^version = ".*"$/m, `version = "${version}"`);
writeFileSync(cargoTomlPath, updatedCargoToml);
