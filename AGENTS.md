# status-comment

## Docs describe the current state

Docs, ADRs, code comments, help text and tests describe how things work now. Leave out predecessor tools, old syntax, past typos and "used to" phrasing. Leave out design-discussion leftovers too: caveats that only make sense to someone who saw a rejected idea (e.g. "--step-name is never read from the environment"). In ADRs, describe rejected options on their own merits rather than by who used them.

## Names and keys are plain strings

Treat user-supplied identifiers (Step names, Status names, config keys) as plain strings: don't reject characters like `=` and don't normalize aliases. Don't propose input validation or alias normalization for names or keys. If a permissive string has a silent-failure consequence, mention it once (e.g. in a BREAKING CHANGE footer) rather than adding a check.
