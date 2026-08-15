// @ts-check
// Global teardown: kill the spawned task-server + static web server.
// The throwaway data root is kept (it's in $TMPDIR and holds the
// server log + final vault state — useful when a run fails); its
// path is printed so you can inspect or delete it.

const fs = require("fs");
const path = require("path");

const STATE_FILE = path.join(__dirname, ".mp-state.json");

module.exports = async () => {
  if (!fs.existsSync(STATE_FILE)) return;
  const state = JSON.parse(fs.readFileSync(STATE_FILE, "utf8"));
  for (const pid of [state.serverPid, state.webPid]) {
    if (!pid) continue;
    try {
      process.kill(pid, "SIGTERM");
    } catch {
      /* already gone */
    }
  }
  console.log(`[mp-teardown] killed pids ${state.serverPid}, ${state.webPid}; data root kept at ${state.dataRoot}`);
};
