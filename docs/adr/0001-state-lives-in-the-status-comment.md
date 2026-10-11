# State lives in the Status Comment, and Outputs are never stored twice

Every invocation is a separate process, possibly in a different job or on a different runner, so the Run's state has to live somewhere they can all reach. We store it in the Status Comment itself as a hidden JSON block holding Steps and Statuses. A Step's Output is not in that JSON: it exists only in the rendered body, between tool-owned markers, and is read back from there before each rewrite. A comment holds at most about 65k characters, so storing Output twice would halve the space available for it.

## Considered Options

- **A state file in `$RUNNER_TEMP`.** This is what `tf-summary` did. It's simple, but it only lives for one job. A second job that shares a Comment Key starts with empty state and wipes the first job's Steps from the comment.
- **Workflow artifacts or cache.** These work across jobs, but they're slow and heavy for a write on every Step.

## Consequences

- Templates must render Output only through the `output` filter. Output that skips the filter has no markers, so it's lost on the next write.
- Output read back from the comment may already be truncated. That's acceptable, because truncation only ever shortens it.
- Only one writer per Comment Key at a time. Concurrent jobs that share a key will lose updates, so they must be ordered with `needs:`.
- The Template is not part of this state. It lives in a local temp file, so it's set once per job.
- Runs without a PR have no comment, so their state falls back to a local temp file for that job.
