# Status Comment

Keeps one pull request comment up to date with a GitHub Actions job's Steps: each Step's Status and Output. Vocabulary: [CONTEXT.md](./CONTEXT.md). Decisions: [docs/adr](./docs/adr).

## Usage

```bash
# Set the Steps, in order; `name=Display Name` sets a label
status-comment steps set init validate plan rules-check='OPA Rules' apply

# Use your own Template
status-comment config --set template=.github/terraform-comment.md.j2

# Run a command: the Step shows `running`, then the exit code picks its Status
status-comment exec plan --exit-code-map 0=success,1=failure,2=changes 'terraform plan -detailed-exitcode'

# Set a Status directly; piped stdin becomes the Output
status-comment step apply --status running
terraform apply | status-comment step apply

# Close out the Run
status-comment finalize

# Print the stored comment without changing anything
status-comment preview
```

`status-comment --help` lists the commands; `status-comment <command> --help` shows each one's options.

## Commands

| Command | What it does |
|---|---|
| `steps set <name[=Display Name]>...` | Set the Run's Steps, in order. Unlisted Steps are removed; listed ones keep their Status; new ones start `pending`. Reopens a finalized Run. |
| `steps reset` | Set every Step back to `pending` and clear its Output. Reopens a finalized Run. |
| `step <name> [--status <status>]` | Set a Step's Status (default `success`). Piped stdin becomes its Output. Unknown Steps are appended. |
| `exec <name> <command> [--exit-code-map <code=status,…>]` | Mark the Step `running`, run `<command>` under `bash -o pipefail`, set the Status from the exit code. Default map: `0=success`; unmapped codes are `failure`. |
| `finalize` | Close out the Run: `pending` → `skipped`, `running` → `cancelled`; write the job summary. |
| `config list` | Print the current config. |
| `config [--load <file>] [--set <key>=<value>]...` | Change the config and print the result. `--load` replaces it with a JSON file (relative `template` paths resolve against that file's directory); `--set` changes one key. |
| `preview [--target comment\|summary]` | Print the whole stored comment exactly as saved, hidden markers included (default), or the job summary it would produce. Changes nothing. |
| `default-template` | Print the built-in Template. |

Every command also takes `--state-path <dir>` (where `config.json` lives, and `comment.md` when there's no PR; default `$RUNNER_TEMP/status-comment`, or `.` in Local Mode) and `--token <token>` (default `$GITHUB_TOKEN`).

### Config keys

| Key | Meaning |
|---|---|
| `template` | Path to a Template file, saved as absolute. Later edits to the file apply. |
| `comment-key` | Which Status Comment to update. Default: workflow name + job id; `local` in Local Mode. |

## Statuses and exit codes

| Status | Meaning | Set by | Failing |
|---|---|---|---|
| `pending` | Not started | `steps set` for new Steps; `steps reset` | |
| `running` | In progress | `exec` while the command runs | |
| `success` | Finished cleanly | An exit code mapped to it (default `0`); `step` by default | |
| `failure` | Finished with an error | Any unmapped exit code | ✓ |
| `skipped` | Never ran | `finalize`, from `pending` | |
| `cancelled` | Interrupted | `finalize`, from `running` | ✓ |

Any Built-in Status can also be set with `step --status` or `--exit-code-map`; `canceled` means `cancelled`. Anything else is a Custom Status (e.g. `changes`): no meaning to the tool, never Failing, rendered by the Template.

- `exec` exits `0` unless the Status is Failing. An unmapped exit code passes through; a mapped Failing one exits non-zero.
- `step` and `exec` append `status=<status>` to `$GITHUB_OUTPUT`.
- GitHub API errors (fork PRs, missing `pull-requests: write`, rate limits) print a warning annotation and leave the exit code alone.

## How it works

- One Status Comment per Comment Key, found by a hidden marker and updated in place.
- A new run ID starts the Steps fresh; re-run attempts keep them.
- Outputs share one size budget. Failing Steps get priority; truncation keeps the tail.
- The comment and the job summary render from the same Template.
- Without a PR (e.g. on `push`) the comment is `comment.md` in the state path, and `finalize` still writes the job summary.

## Local Mode

When `GITHUB_ACTIONS` isn't `true`, status-comment runs in **Local Mode** so you can try commands and Templates on your machine:

- It never calls GitHub.
- The state path defaults to the current directory, so you get `./comment.md` and `./config.json`. The Comment Key defaults to `local`.
- Each command that changes Steps prints the rendered comment to stderr.
- A Run ends at `finalize`; the next `steps set`, `step` or `exec` starts a new one.

```bash
status-comment steps set lint test deploy
status-comment exec lint 'cargo clippy'
status-comment exec test 'cargo test'
status-comment finalize
```

## Templates

Templates are [minijinja](https://github.com/mitsuhiko/minijinja) (Jinja2) files, rendered without HTML escaping. The default is [`src/default_template.md.j2`](./src/default_template.md.j2).

| Variable | Contents |
|---|---|
| `steps[]` | `name`, `display_name`, `status`, `failing`, `builtin`, `has_output` |
| `run` | `id`, `attempt`, `url`, `failing`, `finalized` |
| `github` | `workflow`, `job`, `actor`, `repository`, `sha`, `short_sha`, `pr_number`, `pr_url`, `server_url`, `updated_at` |
| `target` | `"comment"` or `"summary"` |

Render Output with `{{ step | output }}`, the only way to reach it: the filter's markers are how the next call reads it back ([ADR 0001](./docs/adr/0001-state-lives-in-the-status-comment.md)). Booleans print as `True`/`False`; branch on them with `{% if %}`.

## Data model

The hidden JSON at the bottom of the Status Comment. Simplified from [`src/state.rs`](./src/state.rs) and [`src/status.rs`](./src/status.rs):

```rust
struct State {
    /// A different run ID starts a fresh State. Local Mode uses `local-1`, `local-2`, …
    run_id: String,
    /// Set by `finalize`; cleared by `steps set`, `steps reset`, `step` or `exec`.
    finalized: bool,
    steps: Vec<Step>,
}

struct Step {
    /// Identity, from `steps set`, `step` or `exec`.
    name: String,
    /// Label: from `steps set name=Label`, or the title-cased name.
    display_name: String,
    status: Status,
    /// Merged stdout and stderr. Never serialized: it lives only in the rendered comment.
    output: Option<String>,
}

/// Serialized as a plain string: "success", "changes", …
enum Status {
    Pending,
    Running,
    Success,
    Failure,
    Skipped,
    Cancelled,      // also parses "canceled"
    Custom(String), // e.g. "changes"
}
```

## Caveats

- **`step` can't see upstream failures.** In `terraform plan | status-comment step plan`, terraform's exit code never reaches the tool. Use `exec` when the Status matters.
- **Matrix legs share a job id**, so they share the default Comment Key. Give each leg its own key with `config --set comment-key=<key>`.
- **One writer per Comment Key.** Order jobs that share a key with `needs:`; concurrent writers lose Steps.

## Installation

Not published yet. CI uploads a Linux x86_64 build as the `status-comment-linux-x86_64` artifact. Release binaries and a `setup-status-comment` action are planned.

## Development

The Rust toolchain is pinned with [mise](https://mise.jdx.dev):

```bash
mise install
mise exec -- cargo test
mise exec -- cargo clippy --all-targets
```
