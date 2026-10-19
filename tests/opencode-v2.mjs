import assert from "node:assert/strict";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import * as path from "node:path";
import test from "node:test";

// Load the shipped adapter without installing an SDK or changing its tool bridge.
const source = readFileSync(new URL("../templates/opencode-dinyd-guard.ts", import.meta.url));
const { default: adapter } = await import(`data:text/javascript;base64,${source.toString("base64")}`);
const root = process.argv[2];

for (const duplicateExists of [false, true]) {
  test(`nested session with duplicate directory ${duplicateExists ? "present" : "absent"}`, async () => {
    const subpath = duplicateExists ? "app-existing" : "app-missing";
    const directory = path.join(root, subpath);
    mkdirSync(directory);
    if (duplicateExists) mkdirSync(path.join(directory, subpath));
    const document = "outside\n<!-- dinyd-lock -->\nprotected\n<!-- /dinyd-lock -->\n";
    writeFileSync(path.join(directory, "doc.md"), document);
    let before;
    await adapter.setup({
      location: { directory: root },
      session: { get: async () => ({ location: { directory }, subpath }) },
      tool: { hook: async (_event, handler) => { before = handler; } },
    });
    const edit = (oldText) => before({
      tool: "apply_patch",
      sessionID: "nested-session",
      input: {
        patchText: `*** Begin Patch\n*** Update File: doc.md\n@@\n-${oldText}\n+changed\n*** End Patch`,
      },
    });
    await assert.rejects(() => edit("protected"), {
      message: "doc.md: locked section. Leave it unchanged.",
    });
    await edit("outside");
  });
}
