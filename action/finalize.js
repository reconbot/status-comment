// The action's post step: runs `status-comment finalize` at the end of the job.
// It never fails the job; a problem here becomes a warning.
const { execFileSync } = require("node:child_process");

const bin = process.env.STATE_bin;
if (process.env.INPUT_FINALIZE === "false") {
  console.log("status-comment: finalize: false, skipping");
} else if (!bin) {
  console.log("status-comment: not installed, nothing to finalize");
} else {
  try {
    execFileSync(bin, ["finalize"], { stdio: "inherit" });
  } catch (error) {
    console.log(`::warning::status-comment finalize failed: ${error.message}`);
  }
}
