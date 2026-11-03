// Builds the npm packages from the release archives:
//   node npm/pack.mjs <version> <archive-dir> <out-dir>
import { execFileSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

// dist target → npm os/cpu; keep in step with `targets` in dist-workspace.toml.
// Linux archives are static musl builds, so they declare no libc and serve
// glibc hosts too.
const PLATFORMS = [
  ["aarch64-apple-darwin", "darwin", "arm64"],
  ["x86_64-apple-darwin", "darwin", "x64"],
  ["aarch64-unknown-linux-musl", "linux", "arm64"],
  ["x86_64-unknown-linux-musl", "linux", "x64"],
  ["x86_64-pc-windows-msvc", "win32", "x64"],
];

const [version, archives, out] = process.argv.slice(2);
if (!version || !archives || !out) {
  console.error("usage: node npm/pack.mjs <version> <archive-dir> <out-dir>");
  process.exit(2);
}
const repo = join(dirname(fileURLToPath(import.meta.url)), "..");
const common = {
  version,
  license: "MIT OR Apache-2.0",
  // Provenance requires the repository to match the publishing workflow's.
  repository: { type: "git", url: "git+https://github.com/undervoke/dinyd.git" },
};

function writePackage(dir, manifest, files) {
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, "package.json"), `${JSON.stringify(manifest, null, 2)}\n`);
  for (const file of ["LICENSE-MIT", "LICENSE-APACHE", ...files]) {
    copyFileSync(join(repo, file), join(dir, file));
  }
}

function findFile(dir, name) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) {
      const found = findFile(path, name);
      if (found) return found;
    } else if (entry.name === name) {
      return path;
    }
  }
  return undefined;
}

const optionalDependencies = {};
for (const [target, os, cpu] of PLATFORMS) {
  const name = `@undervoke/dinyd-${os}-${cpu}`;
  const exe = os === "win32" ? "dinyd.exe" : "dinyd";
  const unpacked = mkdtempSync(join(tmpdir(), "dinyd-npm-"));
  if (os === "win32") {
    execFileSync("unzip", ["-q", join(archives, `dinyd-${target}.zip`), "-d", unpacked]);
  } else {
    execFileSync("tar", ["-xJf", join(archives, `dinyd-${target}.tar.xz`), "-C", unpacked]);
  }
  const binary = findFile(unpacked, exe);
  if (!binary) throw new Error(`dinyd-${target}: archive has no ${exe}`);
  const dir = join(out, `dinyd-${os}-${cpu}`);
  writePackage(
    dir,
    {
      name,
      ...common,
      description: `The ${os}-${cpu} binary for @undervoke/dinyd`,
      os: [os],
      cpu: [cpu],
    },
    [],
  );
  copyFileSync(binary, join(dir, exe));
  chmodSync(join(dir, exe), 0o755);
  optionalDependencies[name] = version;
}

writePackage(
  join(out, "dinyd"),
  {
    name: "@undervoke/dinyd",
    ...common,
    description: "Keeps documentation claims tied to the code they restate",
    bin: { dinyd: "dinyd.js" },
    optionalDependencies,
  },
  ["README.md"],
);
copyFileSync(join(repo, "npm", "dinyd.js"), join(out, "dinyd", "dinyd.js"));
