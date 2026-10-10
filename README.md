# Status Comment

Status comment can gleam arguments from the step/job/workflow.

- `workflow-name`
- `comment-key` defaults to workflow-name + job name
- `step-name` defaults to the name of the workflow job's step stripping off any trailing `-comment` to the name

Data Model

```typescript
interface Step {
  name: string;
  displayName: string;
  status: "pending" | string;
  exitCodeStatus: Record<number, string>;
  stdout?: string;
  stderr?: string;
}

interface Config {
  steps: Step[];
}
```

```bash
status-comment --steps 'init,validate,plan,rules-check,apply,party' # creates a table of known steps defaulting to pending state
status-comment --exit-code-map '0:success,1:failure:2:changes' --execute 'terraform plan'

```
