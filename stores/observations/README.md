# observations store

Captures and triages observations during AI-assisted work sessions.

## Fields

- `summary` (text, required) — one-line description
- `body` (text, optional) — extended notes; use `--body-from-file` for multi-line
- `triage.verdict` (enum: T1|T2|T3) — triage outcome
- `triage.notes` (text, optional) — free-form triage notes
- `contract.done_when` (text, required when verdict=T3) — completion criteria
- `contract.scope_in` (text, required when verdict=T3) — in-scope work
- `contract.scope_out` (text, required when verdict=T3) — out-of-scope work
- `tags` (list<text>, optional) — pipe-separated: `"frontend|backend"`

## Lifecycle

`open` → `triaged` (verb: `triage`, actor: ai_with_human)
`triaged` → `resolved` (verb: `resolve`, actor: ai_autonomous)
`triaged` → `wont_fix` (verb: `wont_fix`, actor: ai_with_human)

## Quick start

```bash
stores install ./stores/observations
stores observations add --summary "thing broke"           # returns L001
stores observations triage L001 --verdict T3 \
    --done-when "X works" --scope-in "backend" --scope-out "frontend"
stores observations show L001
stores observations list
CLAUDECODE=1 stores observations resolve L001
```
