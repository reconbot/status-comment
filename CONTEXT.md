# Status Comment

A CLI that keeps a pull request comment up to date with the Steps of a GitHub Actions job.

## Language

**Step**:
One row in a Status Comment: a named unit of work with a Status and optional Output, listable before it runs. Its name is its identity (`rules-check`); its display name, set in the Config or title-cased from the name, is its label (`Rules Check`).
_Avoid_: Stage, row

**Workflow step**:
A GitHub Actions `steps:` entry. Always qualified, so "Step" alone means the row.
_Avoid_: action step

**Output**:
A Step's captured text: stdout and stderr merged in print order.
_Avoid_: stdout, stderr, logs

**Status**:
A Step's current state: a Built-in Status or a Custom Status.
_Avoid_: Outcome, state, result

**Built-in Status**:
A Status the tool understands: `pending`, `running`, `success`, `failure`, `skipped`, `cancelled`. Every Template handles them.

**Cancelled**:
The Status of interrupted work: GitHub reported it cancelled, or it was still `running` at Finalize.
_Avoid_: canceled, stopped, aborted

**Custom Status**:
Any other Status, such as `changes`. The tool gives it no meaning; the Template renders it.

**Failing**:
A Step whose Status is `failure` or `cancelled`. A Run is Failing when any Step is.

**Exit Code Map**:
The rules that turn a command's exit code into its Step's Status.

**Status Comment**:
The one PR comment showing every Step under a Comment Key, rewritten in place.
_Avoid_: Bot comment, summary comment

**Comment Key**:
Which Status Comment an invocation updates. A config setting, defaulting to workflow name + job id.
_Avoid_: Comment ID

**Template**:
The minijinja layout that renders a Status Comment: the Default Template, or a user's file. The Default Template is built in and spelled `default` in the Config.

**Config**:
The per-job settings later invocations read back: the Template, the Comment Key, Step display names and Status labels. Created with every default written in.
_Avoid_: settings, options

**State Path**:
The directory holding the Config, and the Status Comment when there's no PR to post it on.
_Avoid_: state dir, temp dir

**Run**:
One workflow run, identified by its run ID. A Status Comment shows one Run; a new Run starts it fresh, a re-run attempt carries on.
_Avoid_: Build, execution

**Finalize**:
The invocation that closes out a Run: unrun Steps become `skipped`, the overall result is declared, the job summary is written.
_Avoid_: Post-job

**Reset**:
Starting every Step over: back to `pending`, Output cleared, the Run reopened. The Steps themselves stay.
_Avoid_: clear, restart

**Local Mode**:
Running outside GitHub Actions: the Status Comment is previewed, never posted, and a Run ends at Finalize.
_Avoid_: dry run, offline mode
