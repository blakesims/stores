# Skill suggestions

This folder contains **suggested** Claude Code skills that operate on the
stores in this repo. They are NOT auto-installed when you `stores install
<store-package>` — installing a store should never silently change your
`.claude/skills/` directory. That coupling is exactly what the
stores-vs-skills separation is meant to avoid.

To use a skill from here:

```bash
# Per-project install:
cp -r skills/observation:triage ~/projects/my-app/.claude/skills/

# Or symlink (lets you pull updates from this repo):
ln -s "$(realpath skills/observation:triage)" ~/projects/my-app/.claude/skills/
```

A full installer (`stores skills install observation:triage --global`) is on
the backlog; for v0.1 you copy by hand.

## Skill design contract

Every skill in here is meant to be **lite**: a thin pointer to the CLI plus
whatever judgment can ONLY come from a skill (rubrics, decision rules,
escalation patterns). The skill does not embed schema knowledge — it asks the
CLI for that via `--help` and `schema --json`. When the schema changes, the
skill keeps working without edits.

Frontmatter convention:

```yaml
---
name: <namespace:verb>
description: <one-liner>
requires_stores: [observations, gate, ...]
default_invoker: human | ai_autonomous | ai_with_human
user_invocable: true
---
```

`requires_stores` is the load-bearing bit. A future framework hook can verify
on skill load that every named store is installed and refuse to load otherwise
with a clean error. For v0.1 it's documentation.

`default_invoker` tells the agent (and a future skill loader) which actor to
pass on CLI invocations from this skill. Skills override per-call when needed.

## Skill body shape

1. State the goal in one paragraph.
2. Tell the agent how to discover the surface: `stores <store> --help`,
   `stores <store> schema --json`.
3. Encode the JUDGMENT layer — rubrics, decision trees, escalation rules.
   This is what the skill brings that the schema can't.
4. Give the canonical command sequence as a worked example, not as the only
   path. The agent improvises within the rubric; the CLI enforces correctness.

When in doubt, **delete prose, not commands**. The shorter the skill, the
fewer places drift can hide.
