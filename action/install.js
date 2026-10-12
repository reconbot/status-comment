// Installs the status-comment release binary for this runner and puts it on PATH.
// Plain Node, no dependencies: the action runs this file as-is.
const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");

const TARGETS = {
  "linux-x64": "x86_64-unknown-linux-musl",
  "linux-arm64": "aarch64-unknown-linux-musl",
  "darwin-arm64": "aarch64-apple-darwin",
};

async function download(url) {
  for (let attempt = 1; ; attempt++) {
    try {
      const response = await fetch(url);
      if (!response.ok) throw new Error(`HTTP ${response.status} for ${url}`);
      return Buffer.from(await response.arrayBuffer());
    } catch (error) {
      if (attempt === 3) throw error;
    }
  }
}

async function main() {
  const target = TARGETS[`${process.platform}-${process.arch}`];
  if (!target) {
    throw new Error(`status-comment has no build for ${process.platform} ${process.arch}`);
  }

  const repo = process.env.GITHUB_ACTION_REPOSITORY || "reconbot/status-comment";
  const version = process.env.INPUT_VERSION || process.env.GITHUB_ACTION_REF || "";
  const base = /^v\d+\.\d+\.\d+/.test(version)
    ? `https://github.com/${repo}/releases/download/${version}`
    : `https://github.com/${repo}/releases/latest/download`;

  const dir = path.join(process.env.RUNNER_TEMP, "status-comment-bin");
  fs.mkdirSync(dir, { recursive: true });
  const archive = path.join(dir, "status-comment.tar.gz");
  fs.writeFileSync(archive, await download(`${base}/status-comment-${target}.tar.gz`));
  execFileSync("tar", ["-xzf", archive, "-C", dir]);
  fs.rmSync(archive);

  const bin = path.join(dir, "status-comment");
  fs.appendFileSync(process.env.GITHUB_PATH, `${dir}\n`);
  // The post step (finalize.js) reads this back as STATE_bin.
  fs.appendFileSync(process.env.GITHUB_STATE, `bin=${bin}\n`);
  execFileSync(bin, ["--version"], { stdio: "inherit" });
}

main().catch((error) => {
  console.log(`::error::${error.message}`);
  process.exit(1);
});
