---
name: observation:log
description: Quickly log something noticed mid-flow into the observations store. Triages first; only files when the fix isn't a T1 you can do right now.
requires_stores: [observations]
default_invoker: ai_with_human
user_invocable: true
---

# Log an observation

The user noticed something during a working session and wants to capture it.
Aim for **10 seconds**: triage first, then either fix in-session (T1) or file
an entry (T2/T3). Do not investigate.

## Discover the surface

Once per fresh session:

```bash
stores observations --help          # verbs
```

The schema tells you what fields `add` accepts. If it changes, your CLI calls
adapt automatically.

## The judgment

Apply this rubric (it lives here, not in the schema, because it's reasoning,
not data):

| Tier | Trigger | Action |
|------|---------|--------|
| **T1** | ≤2 files, ≤50 LOC, no migrations/deps/config | Propose the fix inline. If user approves, edit + commit. **Do not file.** Commit message is the durable record. |
| **T2** | ≤5 files, ≤200 LOC, single subsystem | File the entry; downstream `/observation:triage` will route to a mini-loop. |
| **T3** | Migrations, cross-subsystem, capability change | File the entry; downstream `/observation:triage` will gather the contract. |
| **Can't tell** | Not enough context to triage confidently | File with `--body "needs triage"` so triage classifies later. |

Override: if the user says "just log it," skip the rubric and file.

## Action

```bash
# T2/T3 capture (default path):
stores observations add \
    --summary "<1-line description>" \
    --source dev \
    --priority normal \
    --captured-at "$(date -I)" \
    --captured-week "$(date +w%V-d%u)" \
    [--body "<extended notes>"] \
    [--contact-id <n>] \
    [--field-name <name>]

# Long descriptions: pipe via stdin
stores observations add --summary - <<< "$(cat <<'TXT'
multi-line summary here
TXT
)"
```

`--summary`, `--source`, `--priority`, `--captured-at`, `--captured-week` are
all required by the schema, but the latter two are mechanical — the
`$(date ...)` defaults above fill them without operator input. No
`--invoker` flag is needed on `add`: none of the fields written here carry
an `actor:` constraint, so the auto-detect default is fine.

The CLI returns the new display_id (e.g. `L042`). Confirm to the user in one
line: `Logged L042 (priority): summary`. Done.

## Rules

- One observation per invocation. No batching.
- Default `--source dev` unless the user says Sentry / dashboard / intake.
- Infer fields from conversation context. Don't ask for what's already said.
- Triage before filing.
- Never call other skills from here.
