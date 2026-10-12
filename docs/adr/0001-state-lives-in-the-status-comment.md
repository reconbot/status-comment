# State lives in the Status Comment; Output is stored once

Every invocation is a separate process, possibly in another job or on another runner, so a Run's state must live somewhere they all reach. It lives in the Status Comment: Steps and Statuses as hidden JSON, each Output only in the rendered body between tool-owned markers, read back before every rewrite. A comment holds about 65k characters; storing Output twice would halve its room.

## Considered Options

- **A state file in `$RUNNER_TEMP`.** Simple, but it dies with the job: a second job sharing the Comment Key starts empty and wipes the first job's Steps.
- **Workflow artifacts or cache.** Cross-job, but slow and heavy for a write per Step.

## Consequences

- Templates reach Output only through the `output` filter; Output rendered any other way has no markers and is lost on the next write.
- Output read back may already be truncated. Truncation only shortens it, so that's safe.
- One writer per Comment Key: concurrent jobs sharing a key lose updates, so order them with `needs:`.
- The Template isn't part of this state. Its path lives in `config.json` in the state path, set once per job.
- With no PR there's no comment; it's written to `comment.md` in the state path instead.
- A comment written by another status-comment version is replaced in place: its State is never read.
