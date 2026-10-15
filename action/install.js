// Installs the status-comment release binary for this runner and puts it on PATH.
// Plain Node, no dependencies: the action runs this file as-is.
const { execFileSync } = require("node:child_process")
const fs = require("node:fs")
const path = require("node:path")

const VERSION = "0.6.1" // x-release-please-version

const TARGETS = {
  "linux-x64": "x86_64-unknown-linux-musl",
  "linux-arm64": "aarch64-unknown-linux-musl",
  "darwin-arm64": "aarch64-apple-darwin",
}

async function download(url) {
  let attempt = 0
  while (true) {
    attempt++
    try {
      const response = await fetch(url)
      if (!response.ok) {
        throw new Error(`HTTP ${response.status} for ${url}`)
      }
      return Buffer.from(await response.arrayBuffer())
    } catch (error) {
      if (attempt === 3) throw error
    }
  }
}

async function main() {
  const target = TARGETS[`${process.platform}-${process.arch}`]
  if (!target) {
    throw new Error(`status-comment has no build for ${process.platform} ${process.arch}`)
  }

  const repo = process.env.GITHUB_ACTION_REPOSITORY
  const url = `https://github.com/${repo}/releases/download/v${VERSION}/status-comment-${target}.tar.gz`

  const dir = path.join(process.env.RUNNER_TEMP, "status-comment-bin")
  fs.mkdirSync(dir, { recursive: true })
  const archive = path.join(dir, "status-comment.tar.gz")
  let tarball
  try {
    tarball = await download(url)
  } catch (error) {
    throw new Error(`status-comment v${VERSION} for ${target} could not be downloaded: ${error.message}`)
  }
  fs.writeFileSync(archive, tarball)
  execFileSync("tar", ["-xzf", archive, "-C", dir])
  fs.rmSync(archive)

  const bin = path.join(dir, "status-comment")
  fs.appendFileSync(process.env.GITHUB_PATH, `${dir}\n`)
  // The post step (finalize.js) reads this back as STATE_bin.
  fs.appendFileSync(process.env.GITHUB_STATE, `bin=${bin}\n`)
  execFileSync(bin, ["--version"], { stdio: "inherit" })

  const configPath = process.env["INPUT_CONFIG-PATH"] || ""
  if (configPath) {
    execFileSync(bin, ["config", "load", configPath], { stdio: "inherit" })
  }
  // After the load, which would replace it.
  const token = process.env.INPUT_TOKEN || ""
  if (token) {
    console.log(`::add-mask::${token}`)
    execFileSync(bin, ["config", "set", `token=${token}`], { stdio: "inherit" })
  }
}

main().catch((error) => {
  console.log(`::error::${error.message}`)
  process.exit(1)
})
