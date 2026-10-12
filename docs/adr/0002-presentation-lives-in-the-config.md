# Presentation lives in the Config; the State holds only what drives behavior

The State in the Status Comment holds what later calls need to decide what to do: Step names, Statuses and Run bookkeeping. How any of it is shown — Step display names, Status labels, the Template — lives in the Config or the Template. Every byte of State competes with Output for the comment's ~65k characters, and presentation read back from an old comment would override what the current job asked for.

Step display names (`step.<name>`) and Status labels (`status.<status>`) are the worked example: both are Config keys, looked up at render time, falling back to the title-cased name and the Status itself.

## Considered Options

- **Display names in the State.** The comment describes itself, but a label is frozen by whichever call wrote it, and every Step pays for it in the hidden JSON.
- **Labels in the Template.** Works for Status labels, but every label change means copying the whole Template.

## Consequences

- Every render uses the current Config, so a later `config set step.<name>=…` relabels Steps already in the comment.
- The Config is per job. Jobs sharing a Comment Key each need the same labels; `config load` a checked-in file in every job.
- Built-in Status labels are written into the Config as defaults, so `config list` shows them; emptying one restores the default.
