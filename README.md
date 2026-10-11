# Status Comment

A CLI for GitHub Actions that keeps a single pull request comment up to date with the Steps of a job: their Status and their Output. See [CONTEXT.md](./CONTEXT.md) for the vocabulary and [docs/adr](./docs/adr) for key decisions.

## Usage

```bash
# Declare Steps up front; they start as `pending`
status-comment --steps 'init,validate,plan,rules-check,apply'

# Set the Template for this job (a minijinja file; the built-in default is used otherwise)
status-comment --template .github/terraform-comment.md.j2

# Run a command: marks the Step `running`, captures stdout+stderr as one Output,
# then maps the exit code to a Status
status-comment --step-name plan --exit-code-map '0=success,1=failure,2=changes' --execute 'terraform plan -detailed-exitcode'

# Pipe Output in; Status is `success` unless --status is given
echo foo | status-comment --step-name foo
echo bar | status-comment --step-name bar --status party

# Close out the Run: pending → skipped, running → cancelled, banner and job summary
status-comment --finalize
```

## Flags

| Flag | Meaning |
|---|---|
| `--step-name <name>` | Required for any call that records a Step. Never read from the environment. |
| `--display-name <label>` | The label shown for the Step. Defaults to title-casing the name (`rules-check` → `Rules Check`). |
| `--steps <a,b,c>` | Declares Steps as `pending`. Later declarations merge with earlier ones and keep the first order. Steps that were never declared are appended at the end. |
| `--status <status>` | Status for piped mode. `canceled` is accepted as an alias of `cancelled`. |
| `--execute <cmd>` | Runs `cmd` under `bash -o pipefail` and records its Output and Status. |
| `--exit-code-map <code=status,...>` | Maps exit codes to Statuses. Defaults to `0=success`. Any unmapped exit code records `failure`. |
| `--template <file>` | Sets the Template. It's stored in a local temp file, so set it once per job. |
| `--comment-key <key>` | Which Status Comment to update. Defaults to workflow name + job id. |
| `--finalize` | Closes out the Run. |
| `--token <token>` | Overrides the `GITHUB_TOKEN` env var. |

## Statuses

The Built-in Statuses are `pending`, `running`, `success`, `failure`, `skipped` and `cancelled`, and every Template must handle them. Any other value is a Custom Status (e.g. `changes`), which the Template renders however it likes.

A Step is **Failing** when its Status is `failure` or `cancelled`. Custom Statuses are never Failing.

## Exit codes and outputs

- `--execute` exits non-zero when the resulting Status is Failing. If the exit code wasn't in the Exit Code Map, it exits with the command's own code. Otherwise it exits `0`.
- Every call that records a Step writes `status=<status>` to `$GITHUB_OUTPUT`.
- If the GitHub API can't be reached (e.g. fork PRs, missing `pull-requests: write`, rate limits), the tool prints a warning annotation and the exit code is unaffected.

## Caveats

- **Piped mode can't see failures upstream in the pipe.** In `terraform plan | status-comment …` the tool never sees terraform's exit code. Use `--execute` when the Status matters.
- **Matrix jobs share a job id**, so every leg uses the same default Comment Key. Give each leg its own `--comment-key`.
- **One writer per Comment Key at a time.** Jobs that share a key must be ordered with `needs:`, or one job's Steps will be lost.
- **Templates must render Output through the `output` filter.** The comment is the only copy of each Step's Output, and the filter's markers are how it's read back (see [ADR 0001](./docs/adr/0001-state-lives-in-the-status-comment.md)).
- **Without a PR** (e.g. on `push`), no comment is written. `--finalize` still writes the job summary.

## Behaviour

- One Status Comment per Comment Key. It's updated in place, and found by a hidden marker.
- A new Run (a new run ID) resets the Steps. Re-run attempts of the same Run keep them.
- Output shares one size budget across the comment. Failing Steps get priority, and truncation keeps the tail.
- The comment and the job summary are rendered from the same Template.

## Data model

```typescript
type BuiltInStatus = "pending" | "running" | "success" | "failure" | "skipped" | "cancelled";

interface Step {
  name: string;          // identity, from --step-name
  displayName: string;   // label, title-cased name unless --display-name
  status: BuiltInStatus | string;
  output?: string;       // merged stdout+stderr; lives only in the rendered comment
}

interface State {        // hidden JSON in the Status Comment
  runId: string;
  steps: Step[];         // without output
}
```

## Templates

Templates are [minijinja](https://github.com/mitsuhiko/minijinja) (Jinja2) files. Nothing is HTML-escaped. The built-in default is [`src/default_template.md.j2`](./src/default_template.md.j2). A Template can use:

| Variable | Contents |
|---|---|
| `steps[]` | `name`, `display_name`, `status`, `failing`, `builtin`, `has_output` |
| `run` | `id`, `attempt`, `url`, `failing`, `finalized` |
| `github` | `workflow`, `job`, `actor`, `repository`, `sha`, `short_sha`, `pr_number`, `pr_url`, `server_url`, `updated_at` |
| `target` | `"comment"` or `"summary"` |

Render Output with `{{ step | output }}`. There is no `step.output`, on purpose. Booleans print as `True`/`False`, so test them with `{% if %}` rather than printing them.

## Installation

A prebuilt binary is published on GitHub Releases, along with a `setup-status-comment` action that downloads it.

## Development

The Rust toolchain is pinned with [mise](https://mise.jdx.dev):

```bash
mise install
mise exec -- cargo test
mise exec -- cargo clippy --all-targets
```
