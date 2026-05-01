# CLI smoke test — 2026-04-26

## Summary
- ~30 distinct commands run across init / install / add / show / list / update / triage / resolve / wont_fix / gate answer / skills / manifest corruption.
- 7 issues found: **2 critical**, **3 major**, **2 minor / nit**.
- **Headline finding (CRITICAL):** transition-level `actor` constraints in `observations` are silently ignored — `triage` and `wont_fix` are declared `actor: ai_with_human` in `stores/observations/schema.yaml`, but the CLI happily executes both as `ai_autonomous` (auto-detected via `CLAUDECODE=1`) without warning, error, or even a metadata flag. The same enforcement layer correctly rejects `gate answer` (declared `actor: human`), so the bug is specifically that `ai_with_human` transitions accept `ai_autonomous` as if they were unguarded. Field-level `actor: human` (gate.answer) works; transition-level `actor: ai_with_human` does not.
- Demo path (init → install both → add → triage → resolve, plus gate add → gate answer with `--invoker human`) works end-to-end. Cross-store JOIN via raw sqlite works. JSON output is well-formed and `jq`-clean.

## What works (briefly)
- `stores init`, `stores install <path>` for both bundled stores; manifest is human-readable.
- `add` with normal/empty/very-long/special-char/multi-line-via-`--summary-from-file -` payloads — all stored verbatim, unicode preserved, multiline preserved in JSON output.
- `--json` output is valid JSON; nested Records (`triage`, `contract`) actually nest; `jq .` parses cleanly.
- State machine: triaging an already-triaged row, resolving an `open` row, etc. all hit a clear `cannot <verb>: row is in state 'X', expected 'Y'` error.
- `required_when: "triage.verdict == 'T3'"` enforcement is exact: with no contract fields, all three sub-fields are reported missing with the rule cited; with two of three, only the missing one is reported.
- `gate answer G001` under `CLAUDECODE=1` correctly fails with a verbose, fix-suggesting error pointing at `--invoker human`. Passing `--invoker human` then succeeds. `--invoker ai_with_human` is rejected (gate.answer demands literal `human`).
- Reinstall of the same store from the same path is rejected with a clear "already installed; v0.1 has no migrations" message.
- Sub-field update (`update OBS006 --notes ...`) preserves sibling sub-fields on the same Record; List fields (`tags`) are *replaced* on update, not appended.
- Skills install/uninstall lifecycle: `skills list`, `skills install observation:log` lands at `.claude/skills/observation:log/SKILL.md`, `skills uninstall` removes it cleanly.
- Cross-store coexistence: both tables in `.stores/db.sqlite`, raw `JOIN observations o ON g.task_ref = o.display_id` works, no surprising interactions.
- ID format is strict: `obs001`, `OBS1`, `G001` (wrong store) all error correctly with `no entry with display_id '...'`.

## Issues found

### CRITICAL: transition `actor: ai_with_human` not enforced
**Repro:**
```
export CLAUDECODE=1   # forces auto-detect to ai_autonomous
stores observations add --summary "actor probe"   # OBSnnn
stores observations triage OBSnnn --verdict T1    # schema says actor: ai_with_human
stores observations wont_fix OBSnnn               # schema says actor: ai_with_human
```
**Got:** Both transitions succeed with `updated_by=ai_autonomous`. No error, no warning.
**Expected:** Same enforcement model as `gate answer` — reject with a message pointing at `--invoker ai_with_human`. Or, if `ai_autonomous` is intentionally allowed to satisfy `ai_with_human`, document that semantic explicitly (it is currently undocumented and surprising).
**Why this matters:** The whole "human-in-the-loop" guarantee for the observations store is decorative. An autonomous agent under `CLAUDECODE=1` can triage and close-as-wont-fix without any human ever being involved, despite the schema saying otherwise.

### CRITICAL: `--invoker <bogus>` silently accepted
**Repro:**
```
stores observations add --summary "x" --invoker zorblax     # succeeds, created_by=ai_autonomous
stores observations add --summary "x" --invoker ""          # succeeds, created_by=ai_autonomous
```
**Got:** Unknown invoker values are silently swallowed and the default (auto-detected) is used. No validation error.
**Expected:** Reject with `invalid --invoker value 'zorblax'; expected one of: human | ai_autonomous | ai_with_human` (the same string the help already prints).
**Why this matters:** A user who fat-fingers `--invoker huamn` will think they ran as a human, but every row will silently be attributed to `ai_autonomous`. Combined with the previous critical, this produces a *very* misleading audit trail.

### MAJOR: `--tags` has no documented separator and treats commas as part of a single tag
**Repro:**
```
stores observations add --summary "tag test" --tags "tag1,tag2"
stores observations show OBSnnn --json | jq '.tags'
# => ["tag1,tag2"]
```
**Got:** Single-element list with a literal comma inside. The pipe form `--tags "alpha|beta"` does parse correctly into `["alpha","beta"]`.
**Expected:** Either (a) document the pipe convention in `--tags` help (it *is* documented for `gate.options`: `"Possible answers; pipe-separated on CLI: 'soft|hard'"`), or (b) accept comma-separated, or (c) require repeated flags. Right now `tags` help says nothing about how to enter multiple values, and the comma form silently produces a malformed value.
**Why this matters:** Inconsistent CLI ergonomics within the same binary; users will discover this only via JSON inspection.

### MAJOR: Per-verb `--help` lists every field as if it applied to that verb
**Repro:** `stores observations resolve --help` shows `--summary`, `--body`, `--verdict`, `--done-when`, `--scope-in`, `--scope-out`, `--tags`, etc.
**Got:** Every verb (`add` / `update` / `triage` / `resolve` / `wont_fix`) renders an identical option list — the entire field surface of the store, regardless of which fields the verb is supposed to set.
**Expected:** Each verb's help shows only the fields that verb meaningfully writes. As-is, the help cannot be used to discover which flags are sensible per verb; the user has to read the schema YAML.
**Why this matters:** Discoverability collapses; new users will be confused why `resolve --verdict T3` is offered (it has no effect) and why every verb advertises `--summary` (only `add` requires it, others quietly accept it as an in-place mutation).

### MAJOR: `--summary-from-file` against a missing path is masked as "summary: required"
**Repro:**
```
stores observations add --summary-from-file /tmp/no-such-file
# => Error: validation failed:
# => - summary: required
```
**Got:** Validation error claims summary is required.
**Expected:** I/O error: `cannot read '/tmp/no-such-file' for --summary-from-file: No such file or directory`. The validation layer fires after the file-read silently failed, hiding the real cause.
**Why this matters:** Diagnosing a wrong path takes much longer than it should; the user reads the error as "I forgot to pass `--summary`" when in fact they passed it but the file was missing.

### MINOR / nit: plain `list` output is mangled by multiline `summary` values
**Repro:** `add --summary-from-file -` with `printf "line one\nline two\nline three"`, then `stores observations list`.
**Got:** Output for that row spans multiple terminal lines because `summary=line one\nline two\nline three` is rendered raw, breaking the otherwise per-row layout.
**Expected:** Either truncate-with-ellipsis at the first newline, or quote/escape, or wrap each row.
**Why this matters:** Human readability of `list` is brittle; one rogue multiline row corrupts the whole table.

### MINOR / nit: skill install path uses `:` in a directory name
**Repro:** `stores skills install observation:log` creates `.claude/skills/observation:log/`.
**Got:** Works on Linux ext4; would fail on Windows and on case-/colon-restricted filesystems.
**Expected:** Sanitize to `observation-log` or `observation/log`. Not urgent, but a portability landmine.

## Confusing UX (no bugs, but worth noting)
- `update --status resolved` is a perfectly natural thing for a user to try (it shows up in the observation lifecycle), and the rejection message is helpful (`tip: a similar argument exists: '--tags'`) — but the *real* answer is "use the lifecycle verbs `triage` / `resolve` / `wont_fix`, you cannot mutate `status` directly." The error doesn't say that; it suggests `--tags`, which is unrelated. A schema-aware hint (`status is managed by lifecycle verbs; use 'stores observations resolve <id>'`) would close the loop.
- `--invoker` is documented identically on every subcommand as "Override actor detection". Combined with the silent-accept bug above, a misspelling here is invisible. A short phrase like `default: auto-detected from $CLAUDECODE` in the help would set expectations.
- `add` with no `--summary` errors with `summary: required`, but `add --summary ""` (empty string) *succeeds* and creates `OBSnnn` with an empty summary. Schema declares `required: true`. Either presence-only validation is intentional, or empty strings should also be rejected.
- Manifest corruption (`stores observations list` after hand-editing `.stores/manifest.yaml`) yields raw serde errors like `mapping values are not allowed in this context at line 4 column 15` and `missing field 'stores'`. Functional, but doesn't suggest the fix (e.g. "manifest at .stores/manifest.yaml is malformed; restore from backup or re-run `stores init` in a fresh dir"). If the manifest is *deleted* entirely, the binary stops recognizing the `observations` / `gate` subcommands at all (`unrecognized subcommand`), which is a non-obvious failure mode.

## Tests run (raw log)
1. `stores --help` — OK, top-level matches what's installed (init/install/skills + per-store).
2. `stores init` (fresh tmpdir) — creates `.stores/db.sqlite` (WAL) + `manifest.yaml`. OK.
3. `stores install /…/observations` + `…/gate` — both install; manifest is clean YAML.
4. `stores observations --help`, `add --help`, `triage --help`, `resolve --help`, `update --help` — note: every per-verb help is identical (MAJOR #2).
5. `stores observations add` (no `--summary`) — rejects: `summary: required`.
6. `stores observations add --summary ""` — *succeeds* (empty string allowed). `OBS001`.
7. `stores observations add --summary "First observation - hello world"` — `OBS002`.
8. `add --summary <1000 'A's>` — `OBS003`, no truncation.
9. `add --summary 'Special: "quotes" \`backticks\` $vars éñü 🎉'` — `OBS004`, preserved in JSON.
10. `printf "line one\nline two\nline three" | stores observations add --summary-from-file -` — `OBS005`, newlines preserved (but garble plain `list`, MINOR #1).
11. `add --summary "T3 test" --verdict T3` — rejected with all three contract sub-fields cited. ✓
12. `show OBS001` (yaml form) and `show OBS999` / `show G001` / `show obs001` / `show OBS1` — all error with `no entry with display_id '…'` (case-sensitive, zero-padding required). ✓
13. `list` (plain) — readable until OBS005 wraps.
14. `list --json | jq .` — valid JSON, all 5 rows. ✓
15. `triage OBS006 --verdict T1 --notes "ignore this one"` — `open → triaged`, `triage.verdict = T1`, no contract demanded. ✓
16. `triage OBS006 --verdict T2` again — `cannot triage: row is in state 'triaged', expected 'open'`. ✓
17. `resolve OBS006` — `triaged → resolved`. ✓
18. `resolve OBS001` (still open) — `cannot resolve: row is in state 'open', expected 'triaged'`. ✓
19. `triage OBS001 --verdict T3 --done-when "DW" --scope-in "SI"` — rejects `scope_out` only. ✓
20. `triage OBS001 --verdict T3` (full contract) → `wont_fix OBS001` — both succeed under CLAUDECODE=1 / `ai_autonomous`, despite schema demanding `ai_with_human`. (CRITICAL #1)
21. `update OBS002 --summary "updated summary" --tags "tag1,tag2"` — succeeds; tags becomes `["tag1,tag2"]` (MAJOR #3).
22. `update OBS999 --summary "nope"` — `no entry with display_id 'OBS999'`. ✓
23. `update OBS002 --status resolved` — `unexpected argument '--status'`, suggests `--tags`. (UX nit)
24. `add --summary "tag pipe test" --tags "alpha|beta"` → tags = `["alpha","beta"]`. ✓
25. `update OBS006 --notes "appended note?"` — sub-field write preserves siblings (`verdict: T1` retained). ✓
26. `update OBS007 --tags "gamma|delta"` — list *replaces*, not appends. (Reasonable default.)
27. `triage OBS010 --verdict T9` — `value 'T9' is not one of the allowed values: [T1, T2, T3]`. ✓
28. `add --summary "..." --invoker zorblax` and `--invoker ""` — both silently succeed with `created_by=ai_autonomous`. (CRITICAL #2)
29. `add --summary-from-file /tmp/no-such-file` — error: `summary: required` (masks I/O error, MAJOR #5).
30. `gate add --type decision --question "Pick one" --options "a|b"` → `G001`; `options` correctly parsed to `["a","b"]`.
31. `gate answer G001 --answer "a"` under CLAUDECODE=1 — rejects with verbose, fix-suggesting message naming `--invoker human`. ✓
32. `gate answer G001 --answer "a" --invoker human` — `pending → answered`. ✓
33. `gate answer G002 --answer "a" --invoker ai_with_human` — rejected (gate.answer requires literal `human`). ✓
34. Cross-store JOIN: `gate add … --task-ref OBS001` then `sqlite3 .stores/db.sqlite "SELECT … FROM observations o JOIN gate g ON g.task_ref = o.display_id"` — returns the join row. ✓
35. `stores install /…/observations` (already installed) — rejected: `already installed from this path; v0.1 has no migrations`. ✓
36. `stores install /tmp/nonexistent-12345` — `cannot resolve path … (os error 2)`. ✓
37. `stores install /tmp` (no schema.yaml) — `cannot read '/tmp/schema.yaml' (os error 2)`. ✓
38. `stores skills list` — lists 4 bundled skills.
39. `stores skills install observation:log` — lands at `.claude/skills/observation:log/SKILL.md`. (MINOR #2 portability nit.)
40. `stores skills uninstall observation:log` — file gone.
41. Manifest corrupt → serde error; manifest empty → `missing field 'stores'`; manifest deleted → `unrecognized subcommand 'observations'`. (UX confusing, not a bug.)
