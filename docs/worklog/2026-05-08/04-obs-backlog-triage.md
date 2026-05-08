# Obs Backlog Triage

**Date:** 2026-05-08
**Type:** note

## Summary

Triage pass over the open observation backlog (357 rows where `status='open'`; the prompt's "372" count was a higher upper bound, the precise filter in this note is `status='open'`). The vast majority of the bloat is the deploy-blocked merge-conflict cascade: **317 of 357 open obs (≈89%)** are auto-filed `deploy-blocked: merge conflict` rows tied to 25 tasks that are now ALL in terminal states (`closed_out_of_band`, `schema_migrated`, `accepted`, `abandoned`). Every row in the cascade can close — the work is on main.

**Recommended closures across buckets:**
- DUPLICATE_OF (cascade fold): **292 rows** fold into 25 keepers, one keeper per task_id.
- SUPERSEDED_BY_T### (intent shipped): **15 rows** (mostly the 25 keepers minus a few that match nothing-shipped + 11 non-cascade obs whose intent has shipped per `engine-health.md`).
- RESOLVED_OUT_OF_BAND (direct main commit): **1 row** (L130).
- WONT_FIX: **1 row** (L078, "test draft").
- ABANDONED_T0_MISFILED: **0 rows** (none of the open backlog is pure-doctrine T0).
- KEEP_NEEDS_RATIFICATION: **5 rows** (L154, L155, L156, L157, L480 — drafts/ready awaiting U1).
- KEEP_ACTIONABLE: **18 rows** (genuinely-open work, including L116 + L122 + L172 + L173 + L195 + L482 + L481).

**Top 3 highest-leverage closures:**
1. **Cascade fold (292 rows)** — `deploy-blocked: merge conflict` dupes against 25 already-terminal tasks (T029/T032/T033/T034/T035/T036/T038/T042/T048/T049/T050/T051/T052/T053/T057/T058/T081/T084/T085/T086/T088/T093/T095/T096). Single biggest signal-noise win in the backlog. Every keeper itself is then `superseded` by its task's terminal state, cascading 25 more closures out of the keeper set. Net: 317 closures in two stages.
2. **Auto-promote / T1 cycle batch supersede (8 rows)** — L108, L109, L116, L117, L119, L122, L123, L126, L130 are all from the May-5/May-6 dogfood-pain era. Per `engine-health.md`, 7 of these have shipped fixes (T039, T054, T055, T060, T067, direct-on-main); only L108 (retroactive tier_hint update) and L116/L122 remain genuinely-open per the engine-health doc.
3. **CLI corruption / fail-silent (2 rows)** — L181, L183 superseded by T076/L184 (private install path) + T066/L176 (self-reexec) + T075/L182 (candidate validation). Engine-health Layer 1 / Layer 4 confirms the surface is closed.

The note's classifications are conservative — when engine-health does not unambiguously say `✅ Tnnn`, the obs goes to KEEP rather than into a drop bucket.

## Methodology

- **Schema confirmation:** `sqlite3 .stores/db.sqlite ".schema observations"`. Confirmed columns (`display_id`, `status`, `summary`, `body`, `task_id`, `intent_contract`, etc.). `status` enum includes `open`, `resolved`, `wont_fix`, `ready`, `investigating` — used `status='open'` as the open-backlog filter (yielded 357 rows; the 17 `ready` rows are observations awaiting auto-promote, not open friction, so they are excluded from this triage).
- **Cascade detection:** `SELECT … WHERE status='open' AND summary LIKE '%deploy-blocked%' GROUP BY task_id` returned 25 task_ids covering 317 rows. Cross-referenced each `task_id` against `tasks.status`; all 25 are terminal.
- **Shipped-task cross-reference:** read `docs/engine-health.md` end-to-end (Layers 1–8 + "Recently shipped" table). Every observation referenced as `✅ Tnnn` was treated as shipped; `⚪ Tn` rows were treated as open per the doc's own statement.
- **Per-row body sampling:** for the 40 non-cascade open obs, pulled `summary || substr(body, 1, 200..250)` and matched the friction text against engine-health's tables to bucket each row.
- **Contract state inspection:** `json_extract(intent_contract, '$.contract_state')` and `tier_hint` to identify draft / ready rows (KEEP_NEEDS_RATIFICATION).

## Bucket: DUPLICATE

Fold each task's cascade dupes into the lowest-numbered keeper L for that task_id. After the fold, each keeper itself is then resolved/superseded against the task's terminal state (see SUPERSEDED bucket). Captured-at column omitted for brevity; all dupes share the same task_id and the same `deploy-blocked: task TNNN merge conflict on branch …` summary.

| keeper L | task | dupe Ls (count) | rationale |
|---|---|---|---|
| L096 | T029 (schema_migrated) | L099, L101, L103, L104, L105 (5) | same task, same `deploy-blocked` summary, captured within minutes of each other 2026-05-05 |
| L097 | T032 (closed_out_of_band) | — (0) | only one row; no dupes — keeper itself goes to SUPERSEDED |
| L098 | T033 (schema_migrated) | L100, L102, L106 (3) | same-task cascade dupes |
| L111 | T034 (abandoned) | L112, L162 (2) | same-task cascade dupes |
| L114 | T035 (closed_out_of_band) | L115, L127, L128 (3) | same-task cascade dupes |
| L118 | T036 (schema_migrated) | — (0) | only one row |
| L125 | T038 (closed_out_of_band) | L139, L140 (2) | same-task cascade dupes |
| L129 | T042 (abandoned) | — (0) | only one row |
| L146 | T048 (schema_migrated) | L147 (1) | same-task cascade dupe |
| L148 | T049 (schema_migrated) | — (0) | only one row |
| L152 | T050 (closed_out_of_band) | — (0) | only one row |
| L159 | T051 (closed_out_of_band) | L160 (1) | same-task cascade dupe |
| L153 | T052 (schema_migrated) | — (0) | only one row |
| L158 | T053 (schema_migrated) | L166 (1) | same-task cascade dupe |
| L167 | T057 (schema_migrated) | — (0) | only one row |
| L168 | T058 (schema_migrated) | — (0) | only one row |
| L189 | T081 (accepted) | L190 (1) | same-task cascade dupe |
| L211 | T084 (closed_out_of_band) | L232,L238,L244,L253,L262,L268,L283,L291,L300,L314,L322,L337,L347,L354,L365,L379,L387,L393,L402,L408,L416,L420,L426,L435,L441,L449,L453,L463,L469,L478 (30) | overnight T086-cascade tail |
| L205 | T085 (closed_out_of_band) | L209,L212,L215,L218,L222,L225,L229,L233,L235,L241,L243,L246,L252,L254,L258,L261,L264,L272,L275,L277,L282,L284,L286,L290,L299,L302,L306,L309,L311,L313,L318,L325,L327,L331,L333,L336,L341,L343,L346,L353,L357,L360,L363,L367,L369,L372,L374,L376,L380,L383,L386,L388,L394,L398,L401,L403,L407,L409,L413,L417,L424,L428,L430,L432,L436,L445,L448,L452,L455,L459,L461,L465,L468,L471,L475 (75) | overnight T086-cascade tail (largest single group: T085 thrashed 76 cycles) |
| L202 | T086 (closed_out_of_band) | L206,L213,L219,L224,L231,L242,L248,L251,L256,L263,L270,L278,L288,L294,L303,L308,L319,L326,L329,L339,L349,L356,L362,L368,L378,L382,L391,L396,L404,L415,L421,L425,L431,L438,L443,L451,L458,L466,L477 (39) | overnight T086-cascade tail (T086 = the meta task that triggered the cascade) |
| L200 | T088 (closed_out_of_band) | L201,L210,L221,L228,L240,L247,L260,L267,L273,L281,L295,L301,L312,L323,L332,L345,L361,L370,L377,L390,L399,L410,L419,L437,L446,L462,L474 (27) | overnight T086-cascade tail |
| L198 | T093 (closed_out_of_band) | L207,L214,L220,L227,L234,L239,L249,L255,L259,L269,L276,L279,L289,L292,L296,L298,L305,L310,L317,L321,L330,L338,L342,L348,L352,L359,L364,L371,L375,L381,L389,L395,L400,L406,L412,L422,L429,L434,L442,L444,L450,L457,L460,L470,L472,L476 (46) | overnight T086-cascade tail |
| L204 | T095 (closed_out_of_band) | L208,L217,L223,L230,L236,L245,L257,L266,L271,L280,L287,L293,L304,L315,L320,L328,L334,L340,L350,L355,L366,L373,L384,L392,L397,L405,L414,L423,L433,L439,L447,L456,L467,L473 (34) | overnight T086-cascade tail |
| L203 | T096 (closed_out_of_band) | L216,L226,L237,L250,L265,L274,L285,L297,L307,L316,L324,L335,L344,L351,L358,L385,L411,L418,L427,L440,L454,L464,L479 (23) | overnight T086-cascade tail |

Fold-keeper total: 25 keepers, 292 dupes. Keepers themselves move to SUPERSEDED in the next bucket.

## Bucket: SUPERSEDED

Observations whose intent shipped via a substrate task. Resolution string should reference the shipping task display_id.

| L | shipped via | rationale |
|---|---|---|
| L096 | T029 | task accepted/schema_migrated; engine-health `✅ T029` for the underlying L071 (drive runner-exit). The deploy-blocked obs itself is just a stale cascade artifact against a now-terminal row |
| L097 | T032 | task closed_out_of_band; engine-health `✅ T032` for L032/L067/L080 |
| L098 | T033 | task schema_migrated; engine-health `✅ T033` for L038 |
| L111 | T034 | task abandoned; auto-drive-watchdog spam fix shipped (T089) |
| L114 | T035 | task closed_out_of_band; engine-health `✅ T035` for L113 |
| L118 | T036 | task schema_migrated; engine-health `✅ T036` for L020 |
| L125 | T038 | task closed_out_of_band; engine-health `✅ T038` for L043 |
| L129 | T042 | task abandoned (terminal); cascade obs is stale |
| L146 | T048 | task schema_migrated; engine-health `✅ T048` for L137 |
| L148 | T049 | task schema_migrated |
| L152 | T050 | task closed_out_of_band; engine-health `✅ T050` for L134 |
| L159 | T051 | task closed_out_of_band; engine-health `✅ T051` for L144 |
| L153 | T052 | task schema_migrated; engine-health `✅ T052` for L143 |
| L158 | T053 | task schema_migrated; engine-health `✅ T053` for L142 |
| L167 | T057 | task schema_migrated; engine-health `✅ T057` for L132 |
| L168 | T058 | task schema_migrated; engine-health `✅ T058` for L021 |
| L189 | T081 | task accepted; engine-health `✅ T081` for L053 |
| L211 | T084 | task closed_out_of_band (overnight cascade member; merge ceremony done) |
| L205 | T085 | task closed_out_of_band (overnight cascade member; merge ceremony done) |
| L202 | T086 | task closed_out_of_band; engine-health `✅ T086` for L193 (the meta-fix that triggered the cascade) |
| L200 | T088 | task closed_out_of_band (overnight cascade member; merge ceremony done) |
| L198 | T093 | task closed_out_of_band (overnight cascade member; merge ceremony done) |
| L204 | T095 | task closed_out_of_band (overnight cascade member; merge ceremony done) |
| L203 | T096 | task closed_out_of_band (overnight cascade member; merge ceremony done) |
| L070 | — | engine-health Layer 4 still lists L070 as `⚪ —` (open). Despite T046/T024/T031 partial coverage, the conflict-path side-effect drop is NOT closed per engine-health's own audit. **Reclassified to KEEP_ACTIONABLE.** |
| L109 | T039 | engine-health Layer 3 `✅ T039` — T1 drive end-to-end pull shipped |
| L117 | T039 | engine-health Layer 3 covers T1 drive E2E + T060 tier-aware briefs; auto-promote on-entry actions now fire (skip-plan transition reaches ready). The specific symptom in L117's body matches the T039 fix |
| L119 | T055 | engine-health Recently shipped: `T055 …per-role runner/model config… auto-drive respects runner config; closed out-of-band` |
| L123 | T039 | engine-health `✅ T039` — T1 submit-review PASS path is now in the schema |
| L126 | T060 | engine-health `2026-05-06 T060 L169 — tier-aware executor/code-reviewer briefs skip phase decomposition for T1`. "Phase 1 of 0" symptom resolved |
| L177 | T089 | engine-health Recently shipped: T089/L196 — auto-drive-watchdog terminal-state filter |
| L181 | T076 + T066 + T075 | engine-health Layer 4: L182 `✅ T075 (D)` candidate-binary validation; L184 `✅ T076` private install path. The fail-silent corrupted-stub class is closed by these three together |
| L183 | T076 | same as L181: private install path moved daemon's runtime binary off `~/.cargo/bin/stores`; subagent cargo-install no longer corrupts. Engine-health: "the recurring stub-corruption-tricks-self-reexec class" closed |
| L195 | T086 + agents.yaml | verified live: `.stores/agents.yaml` now contains `external_reviews` subscriber blocks. The ship gap is closed |

Note: L070 was initially expected to land in this bucket but engine-health Layer 4 explicitly flags it `⚪ —` (still open). Bumped to KEEP_ACTIONABLE.

## Bucket: RESOLVED_OUT_OF_BAND

| L | shipped via commit | rationale |
|---|---|---|
| L130 | direct on main 2026-05-06 | engine-health Layer 2: `✅ direct — resume routes blocked T2/T3 with plan=null to planning instead of ready (avoids "Phase 1 of 0" deadlock); fixed direct on main during T038 push`. No task; close with `resolution_kind=addressed_by_commit` |

## Bucket: WONT_FIX

| L | rationale | confidence |
|---|---|---|
| L078 | summary = "test draft", body = "draft body". Created during a CLI rehearsal. Nothing actionable. | high |

## Bucket: ABANDONED_T0_MISFILED

None. The remaining open backlog is actionable substrate friction or substrate observability work. Nothing reads as a doctrine-only mis-filing that should have been a `CLAUDE.md` edit.

## Bucket: KEEP_NEEDS_RATIFICATION

These have `intent_contract` in `draft` or `ready` state and are awaiting a U1 ratification (Pi or Blake). DO NOT close.

- **L154** — draft, T1. Gatekeeper rollout phase boundaries. (Tier T1 — could land doctrinally.)
- **L155** — draft, T3. Dedicated `architecture_reviews` store as Phase 3. (Already partly shipped via T077? Check; if shipped, supersede instead of ratify.)
- **L156** — draft, T3. Fast-track execution waits for Check primitive. Engine-health says T063/L135 Check primitive shipped — this contract may now be ready to amend toward implementation.
- **L157** — draft, T3. Cluster-key registry + watch observability (Phase 5 of T045 design). Pairs with L173.
- **L480** — `contract_state=ready`, T3. Cockpit attention-protection observation; auto-promote subscriber should fire within ~5s of ratification per CLAUDE.md.

Caveat on L155: engine-health Layer 8 `✅ T077` for L171 says the architecture_reviews store DID ship. L155's intent may already be SUPERSEDED — flag for human review.

## Bucket: KEEP_ACTIONABLE

Genuinely-open friction or substrate work. Priority hint where the contract or summary makes it obvious; otherwise blank.

- **L002** — P3. Originally about no delete verb; engine-health Layer 8 says `✅ T043` for L002 (closed transitively via tasks abandon). Re-check — likely belongs in SUPERSEDED. Flag for human review.
- **L006** — P3. Observation runner asymmetry (no drive cycle for obs). Engine-health Layer 8 lists it `⚪ T2`. Open.
- **L012** — P3. No agent-context inspector. Engine-health Layer 5 lists `⚪ T3`. Open.
- **L019** — P3. DockerRunner / standardized agent sandboxing. Engine-health Layer 7 `⚪ T3`. Open.
- **L028** — P2. Drive-spawned agents lack verified `/observe` access. Engine-health Layer 3 `⚪ T2`. Open.
- **L035** — P3. Schema-enforced inter-agent context refs. Engine-health Layer 7 `⚪ T3`. Open.
- **L061** — P2. Pre-promotion acceptance precheck. Engine-health Layer 4 `⚪ T2`. Open.
- **L070** — P2. Accept-merge conflict path drops cargo-install + schema-migrate side effects. Engine-health Layer 4 explicitly `⚪ —` (open).
- **L072** — P2. Code-reviewer REPLAN gate dead-ends as blocked. Engine-health Layer 8 `⚪ —`. Open.
- **L076** — P3. Planner emits multi-phase plans for T2 rows; submit-plan rejects with no auto-recovery edge. Engine-health Layer 3 `✅ T027` covers tier-structural cycle but the auto-recovery edge is not specifically closed. Flag for review.
- **L084** — P3. priority conflates schedule + severity. No engine-health entry. Tier hint missing. Likely needs investigation pass.
- **L085** — P3. Dedup is whispered through QA-specific fields; no first-class duplicate_of/merged_into. Highly relevant given the cascade just fired 292 dupe rows. Promote-worthy.
- **L086** — P3. capability vs capability_ids coexist with no documented rule. Tier T1 doc-clarification candidate.
- **L092** — engine-health Layer 8 lists `✅ T044` for L092 (close-out-of-band verb). Likely SUPERSEDED. Flag for review.
- **L108** — P2. Retroactive tier_hint update doesn't re-trigger fire_on_entry follow-ons. Engine-health Layer 2 `⚪ T2`. Open.
- **L116** — P1. Seeder race during agents.yaml hot-reload. Engine-health Layer 1 `⚪ T2`. Open. Called out by name in CLAUDE.md "L116 + L117 interlock" — but L117 has shipped per engine-health, leaving L116 as the lone holdout.
- **L121** — P2. Pi runner has no timeout / liveness check. Engine-health does NOT list this row directly; T079/L186 (engine-runner actionability monitor) partly covers but not specifically Pi-runner timeout. Keep.
- **L122** — P2. Manual `tasks drive` doesn't set drive_pid; auto-drive can race-spawn duplicate. Engine-health Layer 1 `⚪ T2`. Open.
- **L172** — P3. Fast-track auto-execution + L135 Check primitive (P4 of T045 design). Engine-health Layer 8 `⚪ T3` (deferred).
- **L173** — P3. Curated cluster_key registry + watch/observability dashboards (P5 of T045 design). Engine-health Layer 8 `⚪ T3` (deferred). Pairs with cascade-dedup GAP.
- **L180** — P3. Cosmetic T1 follow-up to T064/L175: tighten silent_zombie reason matching. Tier T1.
- **L187** — P2. `stores observations update` CLI ergonomics — silent failures + missing flags. Engine-controller pain. Tier T1/T2.
- **L481** — P2. `observations add` fails on stale-schema DB; binary should self-detect schema drift. Tier T2. Captured today (2026-05-08) — fresh signal.
- **L482** — P2. CLI multi-value flags split repeated values on embedded commas. Tier T2. Captured today.

Total KEEP_ACTIONABLE: 24 entries above (some flagged for human review and might shift to SUPERSEDED on closer look — count of true-actionable likely 18-20).

## Recommended close-out script

Draft for human/engine-controller execution. Token `T` should be sourced from `~/.config/stores/approve.token` for tier-A verbs. Cascade duplicate folds are AI-autonomous (file dedup is engine work, not a U-moment); supersede / wont_fix are also AI-autonomous resolution writes per current schema. Verify exact verb names against your CLI before running — these are draft shapes.

```bash
T=$(cat ~/.config/stores/approve.token)

# ---------------------------------------------------------------
# Stage 1: cascade duplicate folds (292 rows fold into 25 keepers)
# ---------------------------------------------------------------

# T029 (5 dupes)
for L in L099 L101 L103 L104 L105; do
  stores observations duplicate "$L" --of L096 --invoker ai_autonomous
done

# T033 (3 dupes)
for L in L100 L102 L106; do
  stores observations duplicate "$L" --of L098 --invoker ai_autonomous
done

# T034 (2 dupes)
for L in L112 L162; do
  stores observations duplicate "$L" --of L111 --invoker ai_autonomous
done

# T035 (3 dupes)
for L in L115 L127 L128; do
  stores observations duplicate "$L" --of L114 --invoker ai_autonomous
done

# T038 (2 dupes)
for L in L139 L140; do
  stores observations duplicate "$L" --of L125 --invoker ai_autonomous
done

# T048 (1 dupe)
stores observations duplicate L147 --of L146 --invoker ai_autonomous

# T051 (1 dupe)
stores observations duplicate L160 --of L159 --invoker ai_autonomous

# T053 (1 dupe)
stores observations duplicate L166 --of L158 --invoker ai_autonomous

# T081 (1 dupe)
stores observations duplicate L190 --of L189 --invoker ai_autonomous

# T084 (30 dupes)
for L in L232 L238 L244 L253 L262 L268 L283 L291 L300 L314 L322 L337 L347 L354 \
         L365 L379 L387 L393 L402 L408 L416 L420 L426 L435 L441 L449 L453 L463 \
         L469 L478; do
  stores observations duplicate "$L" --of L211 --invoker ai_autonomous
done

# T085 (75 dupes — biggest group)
for L in L209 L212 L215 L218 L222 L225 L229 L233 L235 L241 L243 L246 L252 L254 \
         L258 L261 L264 L272 L275 L277 L282 L284 L286 L290 L299 L302 L306 L309 \
         L311 L313 L318 L325 L327 L331 L333 L336 L341 L343 L346 L353 L357 L360 \
         L363 L367 L369 L372 L374 L376 L380 L383 L386 L388 L394 L398 L401 L403 \
         L407 L409 L413 L417 L424 L428 L430 L432 L436 L445 L448 L452 L455 L459 \
         L461 L465 L468 L471 L475; do
  stores observations duplicate "$L" --of L205 --invoker ai_autonomous
done

# T086 (39 dupes)
for L in L206 L213 L219 L224 L231 L242 L248 L251 L256 L263 L270 L278 L288 L294 \
         L303 L308 L319 L326 L329 L339 L349 L356 L362 L368 L378 L382 L391 L396 \
         L404 L415 L421 L425 L431 L438 L443 L451 L458 L466 L477; do
  stores observations duplicate "$L" --of L202 --invoker ai_autonomous
done

# T088 (27 dupes)
for L in L201 L210 L221 L228 L240 L247 L260 L267 L273 L281 L295 L301 L312 L323 \
         L332 L345 L361 L370 L377 L390 L399 L410 L419 L437 L446 L462 L474; do
  stores observations duplicate "$L" --of L200 --invoker ai_autonomous
done

# T093 (46 dupes)
for L in L207 L214 L220 L227 L234 L239 L249 L255 L259 L269 L276 L279 L289 L292 \
         L296 L298 L305 L310 L317 L321 L330 L338 L342 L348 L352 L359 L364 L371 \
         L375 L381 L389 L395 L400 L406 L412 L422 L429 L434 L442 L444 L450 L457 \
         L460 L470 L472 L476; do
  stores observations duplicate "$L" --of L198 --invoker ai_autonomous
done

# T095 (34 dupes)
for L in L208 L217 L223 L230 L236 L245 L257 L266 L271 L280 L287 L293 L304 L315 \
         L320 L328 L334 L340 L350 L355 L366 L373 L384 L392 L397 L405 L414 L423 \
         L433 L439 L447 L456 L467 L473; do
  stores observations duplicate "$L" --of L204 --invoker ai_autonomous
done

# T096 (23 dupes)
for L in L216 L226 L237 L250 L265 L274 L285 L297 L307 L316 L324 L335 L344 L351 \
         L358 L385 L411 L418 L427 L440 L454 L464 L479; do
  stores observations duplicate "$L" --of L203 --invoker ai_autonomous
done

# ---------------------------------------------------------------
# Stage 2: cascade keepers themselves now resolve as superseded
# (each keeper's underlying task is in a terminal state)
# ---------------------------------------------------------------

stores observations resolve L096 --resolution "shipped via T029 (schema_migrated); cascade artifact closed" --invoker ai_autonomous
stores observations resolve L097 --resolution "shipped via T032 (closed_out_of_band)" --invoker ai_autonomous
stores observations resolve L098 --resolution "shipped via T033 (schema_migrated)" --invoker ai_autonomous
stores observations resolve L111 --resolution "T034 abandoned; cascade artifact closed" --invoker ai_autonomous
stores observations resolve L114 --resolution "shipped via T035 (closed_out_of_band)" --invoker ai_autonomous
stores observations resolve L118 --resolution "shipped via T036 (schema_migrated)" --invoker ai_autonomous
stores observations resolve L125 --resolution "shipped via T038 (closed_out_of_band)" --invoker ai_autonomous
stores observations resolve L129 --resolution "T042 abandoned; cascade artifact closed" --invoker ai_autonomous
stores observations resolve L146 --resolution "shipped via T048 (schema_migrated)" --invoker ai_autonomous
stores observations resolve L148 --resolution "shipped via T049 (schema_migrated)" --invoker ai_autonomous
stores observations resolve L152 --resolution "shipped via T050 (closed_out_of_band)" --invoker ai_autonomous
stores observations resolve L159 --resolution "shipped via T051 (closed_out_of_band)" --invoker ai_autonomous
stores observations resolve L153 --resolution "shipped via T052 (schema_migrated)" --invoker ai_autonomous
stores observations resolve L158 --resolution "shipped via T053 (schema_migrated)" --invoker ai_autonomous
stores observations resolve L167 --resolution "shipped via T057 (schema_migrated)" --invoker ai_autonomous
stores observations resolve L168 --resolution "shipped via T058 (schema_migrated)" --invoker ai_autonomous
stores observations resolve L189 --resolution "shipped via T081 (accepted)" --invoker ai_autonomous
stores observations resolve L211 --resolution "shipped via T084 (closed_out_of_band; overnight cascade)" --invoker ai_autonomous
stores observations resolve L205 --resolution "shipped via T085 (closed_out_of_band; overnight cascade)" --invoker ai_autonomous
stores observations resolve L202 --resolution "shipped via T086 (closed_out_of_band; meta-fix that triggered cascade)" --invoker ai_autonomous
stores observations resolve L200 --resolution "shipped via T088 (closed_out_of_band; overnight cascade)" --invoker ai_autonomous
stores observations resolve L198 --resolution "shipped via T093 (closed_out_of_band; overnight cascade)" --invoker ai_autonomous
stores observations resolve L204 --resolution "shipped via T095 (closed_out_of_band; overnight cascade)" --invoker ai_autonomous
stores observations resolve L203 --resolution "shipped via T096 (closed_out_of_band; overnight cascade)" --invoker ai_autonomous

# ---------------------------------------------------------------
# Stage 3: non-cascade superseded (intent shipped via referenced task)
# ---------------------------------------------------------------

stores observations resolve L109 --resolution "shipped via T039 (T1 drive E2E pull; engine-health Layer 3)" --invoker ai_autonomous
stores observations resolve L117 --resolution "shipped via T039 + T060 (auto-promote on-entry actions + tier-aware briefs)" --invoker ai_autonomous
stores observations resolve L119 --resolution "shipped via T055 (per-role runner/model config)" --invoker ai_autonomous
stores observations resolve L123 --resolution "shipped via T039 (T1 submit-review path)" --invoker ai_autonomous
stores observations resolve L126 --resolution "shipped via T060 (tier-aware executor/code-reviewer briefs)" --invoker ai_autonomous
stores observations resolve L177 --resolution "shipped via T089 (auto-drive-watchdog terminal-state filter)" --invoker ai_autonomous
stores observations resolve L181 --resolution "shipped via T076 + T066 + T075 (private install + self-reexec + candidate validation)" --invoker ai_autonomous
stores observations resolve L183 --resolution "shipped via T076 (private install path; daemon binary moved off ~/.cargo/bin/stores)" --invoker ai_autonomous
stores observations resolve L195 --resolution "verified live: .stores/agents.yaml now contains external_reviews subscriber; ship gap closed" --invoker ai_autonomous

# ---------------------------------------------------------------
# Stage 4: resolved out of band (direct main commit, no task)
# ---------------------------------------------------------------

stores observations resolve L130 --resolution "fixed direct on main during T038 push 2026-05-06; engine-health Layer 2" --invoker ai_autonomous

# ---------------------------------------------------------------
# Stage 5: wont_fix
# ---------------------------------------------------------------

stores observations wont-fix L078 --reason "test draft row from CLI rehearsal; no actionable signal" --invoker ai_autonomous
```

## Open questions for human

1. **L155 / architecture_reviews store** — engine-health Layer 8 lists `✅ T077` for L171 (dedicated architecture_reviews store shipped). L155 (draft contract for the same architectural concern) may be SUPERSEDED rather than KEEP_NEEDS_RATIFICATION. Want to merge / supersede?
2. **L156 / fast-track Check** — engine-health says T063/L135 Check primitive shipped. L156's draft contract was waiting on exactly this. Promote to ratification, or amend toward L172's implementation contract?
3. **L002 + L092** — engine-health Layer 8 lists both as `✅ T043` and `✅ T044` respectively. Confidence is high they should move to SUPERSEDED, but they were filed before the verbs that closed them. Confirm closure?
4. **L076 (multi-phase plan auto-recovery)** — T027 closed the tier-structural shape but the auto-recovery edge from rejection back to planning is not directly listed as shipped. Still open?
5. **L084 (priority vs severity split)** — no tier_hint, no engine-health entry, no contract. Want this drafted into a contract or left as raw signal?
6. **Cascade-dedup subscriber (`GAP-cascade-dedup` in engine-health)** — the L465–L479 + the entire cascade we are folding here is the cost evidence. Should we promote `GAP-cascade-dedup` to a fresh observation now while the friction is fresh, before cleaning up the dupes makes it harder to read the pattern?
7. **L121 (Pi runner liveness/timeout)** — partially covered by T079's heartbeat/redispatch but Pi-specific timeout isn't called out. Keep open or fold into a new "runner-specific-timeout" observation?

## Follow-ups

- After human ratifies the closures, run the close-out script in stages (Stage 1 first; verify the cascade collapses cleanly before Stage 2's keeper resolutions).
- Update `docs/engine-health.md` § "Recently shipped" with a single entry: `2026-05-08 / triage / 317 cascade dupes folded + 11 superseded / first systematic backlog hygiene sweep`.
- Promote the cascade-dedup observation (GAP-cascade-dedup) to a real L### before the audit trail of dupes is fully resolved — the current backlog IS the evidence.
