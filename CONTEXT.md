# Status Comment

A CLI used inside GitHub Actions jobs that keeps a single pull request comment up to date with the progress, status and output of a job's work.

## Language

**Step**:
One row in a Status Comment: a named unit of work with a Status and optionally its captured output. Usually matches a workflow step, but a Step may be declared before any workflow step runs it. Its name is its identity (`rules-check`). Its display name is its label (`Rules Check`).
_Avoid_: Stage, row

**Output**:
The captured text of a Step's work, with stdout and stderr merged in the order they were printed.
_Avoid_: stdout, stderr, logs

**Workflow step**:
A GitHub Actions `steps:` entry. Always say "workflow step" when this is meant, so "Step" on its own never means it.
_Avoid_: Step (unqualified), action step

**Status**:
The current state of a Step. It is either a Built-in Status or a Custom Status.
_Avoid_: Outcome, state, result

**Built-in Status**:
A Status whose meaning the tool knows: `pending`, `running`, `success`, `failure`, `skipped` and `cancelled`. Every Template must handle them.

**Cancelled**:
The Status of a Step whose work was interrupted. GitHub reported it as cancelled, or it was still `running` at Finalize.
_Avoid_: canceled, stopped, aborted

**Custom Status**:
Any Status that isn't built in, such as `changes`. The tool gives it no meaning and leaves it to the Template to render.

**Failing**:
A Step is Failing when its Status is `failure` or `cancelled`. Custom Statuses are never Failing. A Run is Failing when any of its Steps are.

**Exit Code Map**:
The rules that turn a command's exit code into a Status for its Step.

**Status Comment**:
The single PR comment that shows every Step of one Comment Key. Each new run overwrites it in place, rather than adding a new comment.
_Avoid_: Bot comment, summary comment

**Comment Key**:
The identity that decides which Status Comment an invocation updates. It defaults to the workflow name plus the job name.
_Avoid_: Comment ID

**Template**:
The layout used to render a Status Comment. The tool has one generic default Template built in. Users can replace it with their own Template file, such as a Terraform-specific one.

**Run**:
One GitHub Actions workflow run, identified by its run ID. A Status Comment shows exactly one Run at a time. A new Run starts it fresh, while re-run attempts of the same Run carry on from where it left off.
_Avoid_: Build, execution

**Finalize**:
An explicit invocation that closes out a Run's Status Comment. It marks Steps that never ran as `skipped`, declares the overall result, and publishes the job summary.
_Avoid_: Post-job, party
