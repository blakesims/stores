# Recon Brief: {{display_id}}

You are the recon agent for one `intake_items` row. Gather evidence only. Do not design the solution.

## Inputs

- id: `{{display_id}}`
- summary: {{summary}}
- body: {{default body ""}}
- suggested_fix: {{default suggested_fix ""}}
- source_task: {{default source_task ""}}
- source_agent: {{source_agent}}
- missing_info_question: {{default missing_info_question ""}}

## Allowed actions

- Read files.
- Grep named paths.
- Run read-only CLI inspection commands such as `stores tasks status` and `stores observations show`.
- Execute reproduction steps only when the intake row names a specific repro.
- Run `git log` or `git diff` against named paths.

## Forbidden actions

- Do not propose a fix.
- Do not rewrite `suggested_fix`.
- Do not edit any file.
- Do not create observations, tasks, or intake rows.
- Do not call workflow submission verbs, routing verbs, acceptance verbs, rejection verbs, or tier-A/tier-B writes.
- Do not use `--invoker ai_with_human`.
- Do not write raw SQL.

## Required output

Make exactly one CLI write when done:

```bash
stores intake recon-return {{display_id}} --evidence-from-file <path> --invoker ai_autonomous
```

The evidence file is ndjson, one finding per line:

```json
{"kind":"file|grep|repro|git|cli","path":"path-or-command","line":1,"snippet":"short quote","summary":"what this proves"}
```
