#!/usr/bin/env node
// Writes latest.json, the manifest Citadel's updater reads, from the signed release files in a
// directory: every installer that has a matching .sig file becomes an update for the installs of
// that kind. Usage:
//   node scripts/updater-manifest.mjs <dir> <version> <owner/repo> <tag>
import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const [dir, version, repo, tag] = process.argv.slice(2);
if (!dir || !version || !repo || !tag) {
  console.error("usage: updater-manifest.mjs <dir> <version> <owner/repo> <tag>");
  process.exit(2);
}

// The updater looks for "{os}-{arch}-{installer}" first, then "{os}-{arch}".
const rules = [
  [/_amd64\.AppImage$/, ["linux-x86_64-appimage", "linux-x86_64"]],
  [/_amd64\.deb$/, ["linux-x86_64-deb"]],
  [/\.x86_64\.rpm$/, ["linux-x86_64-rpm"]],
  [/_x64-setup\.exe$/, ["windows-x86_64-nsis", "windows-x86_64"]],
  [/_x64_[^_]+\.msi$/, ["windows-x86_64-msi"]],
  [/_universal\.app\.tar\.gz$/, ["darwin-aarch64-app", "darwin-x86_64-app", "darwin-aarch64", "darwin-x86_64"]],
];

const platforms = {};
for (const name of readdirSync(dir).sort()) {
  if (!name.includes(`_${version}_`) && !name.includes(`-${version}-`)) continue;
  const rule = rules.find(([re]) => re.test(name));
  if (!rule) continue;
  const sig = join(dir, `${name}.sig`);
  if (!existsSync(sig)) {
    console.error(`${name} has no signature; leaving it out of latest.json`);
    continue;
  }
  const entry = {
    signature: readFileSync(sig, "utf8").trim(),
    url: `https://github.com/${repo}/releases/download/${encodeURIComponent(tag)}/${encodeURIComponent(name)}`,
  };
  for (const target of rule[1]) platforms[target] = entry;
}

if (!Object.keys(platforms).length) {
  console.error("no signed update files found");
  process.exit(1);
}

const manifest = {
  version,
  notes: `Citadel ${version}. Release notes: https://github.com/${repo}/releases/tag/${tag}`,
  pub_date: new Date().toISOString().replace(/\.\d{3}Z$/, "Z"),
  platforms,
};
writeFileSync(join(dir, "latest.json"), `${JSON.stringify(manifest, null, 2)}\n`);
console.log(`latest.json covers ${Object.keys(platforms).join(", ")}`);
