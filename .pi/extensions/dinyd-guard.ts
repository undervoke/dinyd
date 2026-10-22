// dinyd-guard: managed by 'dinyd init'. Do not edit; re-run 'dinyd init' to refresh.
import { spawnSync } from "node:child_process";
import * as path from "node:path";

const HOST = "pi";
const EDIT_TOOLS = new Set(["edit", "write", "apply_patch"]);
const UNAVAILABLE = "dinyd guard unavailable; install dinyd and run dinyd init.";

// Null allows the call; a string denies it with that reason.
function judge(tool, args, worktree) {
  const payload = JSON.stringify({ tool: String(tool || "unknown"), args: args || {}, worktree }) + "\n";
  let run;
  try {
    run = spawnSync("dinyd", ["guard", "--host", HOST], {
      input: payload,
      encoding: "utf8",
      cwd: worktree,
      timeout: 30000,
    });
  } catch {
    return UNAVAILABLE;
  }
  if (run.error) return UNAVAILABLE;
  let verdict = null;
  try {
    verdict = JSON.parse(String(run.stdout || "").trim());
  } catch {
    verdict = null;
  }
  if (verdict && verdict.block === true) {
    return typeof verdict.reason === "string" && verdict.reason ? verdict.reason : UNAVAILABLE;
  }
  if (run.status !== 0) return UNAVAILABLE;
  if (verdict && verdict.block === false) return null;
  return UNAVAILABLE;
}

export default function dinydGuard(pi) {
  pi.on("tool_call", async (event, ctx) => {
    const tool = (event && event.toolName) || "unknown";
    if (!EDIT_TOOLS.has(tool.toLowerCase())) return;
    const args = (event && (event.args || event.input)) || {};
    const worktree = path.resolve((ctx && ctx.cwd) || process.cwd());
    const reason = judge(tool, args, worktree);
    if (reason) return { block: true, reason };
  });
}
