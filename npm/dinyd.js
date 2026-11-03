#!/usr/bin/env node
"use strict";

// npm installs only the optional dependency whose os/cpu match this machine.
const { spawnSync } = require("node:child_process");
const path = require("node:path");

const platform = `${process.platform}-${process.arch}`;
let exe;
try {
  const manifest = require.resolve(`@undervoke/dinyd-${platform}/package.json`);
  exe = path.join(path.dirname(manifest), process.platform === "win32" ? "dinyd.exe" : "dinyd");
} catch {
  console.error(
    `dinyd: no prebuilt binary for ${platform}; @undervoke/dinyd-${platform} is not installed`,
  );
  process.exit(1);
}

const result = spawnSync(exe, process.argv.slice(2), { stdio: "inherit" });
if (result.error) {
  console.error(`dinyd: cannot run ${exe}: ${result.error.message}`);
  process.exit(1);
}
// A run killed by a signal has no status and must not read as success.
process.exit(result.status ?? 1);
