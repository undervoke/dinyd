// dinyd-guard: managed by 'dinyd init'. Do not edit; re-run 'dinyd init' to refresh.
import { spawnSync } from "node:child_process";
import * as path from "node:path";

const HOST = "opencode";
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

// Session worktree first: the hook may run in the main checkout while the
// agent edits another worktree. Falls back to the plugin load location.
async function sessionWorktree(ctx, input, fallback) {
  const explicit = input && (input.worktree || input.directory);
  if (typeof explicit === "string" && explicit) return explicit;
  const sessionID = input && (input.sessionID || input.sessionId);
  if (typeof sessionID === "string" && sessionID) {
    try {
      const res = await ctx.client.session.get({ path: { id: sessionID } });
      const dir = res && (res.data || res).directory;
      if (typeof dir === "string" && dir) return dir;
    } catch {}
  }
  return fallback;
}

const server = async (ctx) => {
  const loadWorktree = (ctx && (ctx.worktree || ctx.directory)) || process.cwd();
  return {
    "tool.execute.before": async (input, output) => {
      const tool = (input && input.tool) || "unknown";
      if (!EDIT_TOOLS.has(tool.toLowerCase())) return;
      const worktree = path.resolve(await sessionWorktree(ctx, input, loadWorktree));
      const reason = judge(tool, (output && output.args) || {}, worktree);
      if (reason) throw new Error(reason);
    },
  };
};

export default {
  id: "dinyd-guard",
  server,
  setup: async (ctx) => {
    const loadWorktree = (ctx && ctx.location && ctx.location.directory) || process.cwd();
    await ctx.tool.hook("execute.before", async (event) => {
      const tool = (event && event.tool) || "unknown";
      if (!EDIT_TOOLS.has(tool.toLowerCase())) return;
      let worktree = loadWorktree;
      try {
        const session = await ctx.session.get({ sessionID: event.sessionID });
        if (typeof session?.location?.directory === "string") {
          // subpath is project-relative metadata; directory is already the session path.
          worktree = session.location.directory;
        }
      } catch {}
      const reason = judge(tool, (event && (event.input || event.args)) || {}, worktree);
      if (reason) throw new Error(reason);
    });
  },
};
