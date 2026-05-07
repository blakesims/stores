# Heart & Architect

**Path:** `docs/heart-and-architect.md`
**Status:** direction doc — long-term constitutional governance shape. Phase α shipped in T077/L171 as the dedicated `architecture_reviews` typed store; later typed-Heart, doc-diff, and subscriber slices remain deferred.
**Companion docs:** `docs/philosophy.md` (the Heart content), `docs/primitives.md` (the Primitives content), `docs/architecture-coherence.md` (local correctness ≠ architectural coherence — the doctrine that surfaced this layer), `docs/risk-and-cluster-taxonomy.md` (the typed enums the gatekeeper produces), `docs/gatekeeper-design.md` (T045 phase 2 — `architecture_reviews` specced as P3 follow-up).
**Thesis seed:** `docs/worklog/2026-05-07/02-heart-constitution-architect-thesis.md`.

## 1. The doctrine: two meta primitives, three roles

The substrate has exactly **two meta primitives** — two surfaces that are *constitutional* and require human ratification when they change.

| Meta primitive | What it is | Where it lives today |
|---|---|---|
| **Philosophy** (= Heart, = Intent) | What stores IS. What it is FOR. Boundaries. Guiding principles. The two-gate frame. The deeper bet. The "what" and the "why." | `docs/philosophy.md` + satellites (`architecture-coherence.md`, future Heart-store rows) |
| **Primitives** | The typed constituents the substrate composes from to fulfill the Philosophy. Buffer, Transition, Subscriber, Actor, Direction, Schema, Check, Router today. The "with what." | `docs/primitives.md` + the typed-enum specifications that derive from it (`risk-and-cluster-taxonomy.md`) |

Everything else is **not constitutional**:

- `engine-health.md` — operational measurement of the running system. A dashboard, not doctrine.
- `docs/worklog/` — historical record. Ephemeral; promotes into doctrine only when an insight earns it.
- `CLAUDE.md` (root and per-store) — operating manual / agent conventions. Procedural, not amendable doctrine.
- Briefing templates, agent runtime config, schema YAML files — implementation, projection, configuration.

The constitutional / operational distinction is load-bearing. Operational surfaces change every day without ceremony; constitutional surfaces require human ratification because changing them changes what the system IS or what it COMPOSES FROM.

### The three-role authority model

| Role | Domain | Authority |
|---|---|---|
| **Human** | Source of intent. Owns Philosophy. Sole ratifier of all constitutional amendments (Philosophy AND Primitives). | Tier-A (token-mediated) on every amendment. Cannot be delegated. |
| **Architect** | Constitutional governor. Interprets Philosophy autonomously. Drives Primitive work autonomously (organizing, refining, applying, drafting proposals). Cannot ratify amendments to either meta primitive. | Tier-B (autonomous) on interpretations. Drafts amendments for human ratification. |
| **AI workers** (planner, executor, code_reviewer, gatekeeper, …) | Execute within Philosophy + Primitives as currently constituted. | Constrained by per-field actor + tier discipline. Do not interpret doctrine; do not propose amendments; flag friction into intake. |

The cognitive division: the **Human** cares about Philosophy more than about the constituents required to fulfill it. The **Architect** takes Philosophy as given and works on Primitives in service of it — collaboratively, but with autonomous authority over the *work*. The Architect is the system's mechanism for keeping Philosophy and Primitives coherent without burning the Human's attention on every primitive-level decision.

This is the same protect-human-attention principle that grounds the rest of the substrate (`docs/philosophy.md` § *What the substrate is FOR*), applied to constitutional governance.

## 2. The cryptographic moat: interpret vs. amend

The Architect interprets autonomously but **cannot silently shift the constitution**. The mechanism is the same actor-gate machinery the substrate already uses for task acceptance and contract approval — no new authority surface required.

| Ruling kind | What it does | Authority gate |
|---|---|---|
| **interpret** | Apply existing Philosophy / Primitives to a specific cluster, candidate, or contract. Verdicts: `allow_local_fix`, `reframe_contract`, `merge_with_cluster`, `create_primitive_task`, `block_pending_fixes`, `request_human_arch_decision`. | `actor: ai_with_human` — tier-B honor-system. Architect ratifies on its own authority. |
| **amend** | Propose a change to Philosophy or Primitives themselves. Verdict: `propose_doctrine_update` (with doc-diff and `cascade_decisions`). | `actor: human` — tier-A token-mediated. Architect can DRAFT but not RATIFY. The schema rejects an amend-ratify write that lacks a valid approval token. |

The architect's **authority surface** decomposes cleanly:

The architect *can* (autonomously):
- Ratify interpret-rulings on its own authority.
- Issue verdicts on architecture-review candidates from the gatekeeper.
- Set `pending_architecture_review = false` on observation contracts after issuing a verdict.
- Append citations to ratified rulings (precedent accretion).
- Rename / split / merge `cluster_key` registry entries (when L173 ships).
- Draft amendment proposals to Philosophy or Primitives.

The architect *cannot* (ever, even with token):
- Self-amend. The amend-ratify transition is `actor: human` and the Architect is a different actor class.
- Bypass U-moments (acceptance, contract ratification) — those remain unchanged.
- Override Human decisions — Human always supersedes Architect interpretations.
- Silently change Philosophy or Primitives.

The "too meta" boundary: the Architect cannot redefine *what a primitive IS as a meta-concept* (the rules of the game), nor unilaterally add or remove a primitive. They CAN propose specific primitive changes within the existing meta-frame; ratification is human tier-A.

## 3. Software is ever-evolving

A first-order principle worth elevating to constitutional doctrine in its own right:

> The system is not constrained by yesterday's choice. Doctrine moves when evidence justifies it.

This grounds two operational consequences.

### Flexible precedent (rulings are advisory, not binding)

Prior rulings are **advisory, not binding precedent**. A new architect (or the same architect later) re-decides from current doctrine + evidence; supersession is cheap, not a high procedural bar. The substrate is closer to civil-law (statute = doctrine binds; rulings are interpretive aid) than to common-law (precedent binds).

A May 2026 ruling on dispatch-lock cleanup does not bind a July 2026 architect facing a structurally similar concern. The July architect re-decides. If they reach the same conclusion, they cite May's ruling but write their own. If they reach a different conclusion, they file with `supersedes` set; no cascade required because doctrine itself didn't move.

The discipline tax: architects must search prior rulings before issuing new ones, to avoid silent contradiction. Mitigation: `architecture_reviews list --cluster <key>` + render-injection of prior rulings into the architect's brief.

### Cascade-on-amendment (when doctrine itself moves)

When **Philosophy or Primitives amends**, the architect must enumerate affected prior rulings as part of the amendment draft. For each affected ruling, the architect proposes one of:

- **`re-affirm`** — the ruling stands; its substance is still correct under amended doctrine. Ruling stays `ratified`.
- **`supersede`** — the ruling is no longer correct; supersede with a new interpret-ruling. Original transitions to `superseded`.
- **`leave-flagged-stale`** — the ruling is historical; tag it with `cited_doctrine_amended` and let it stand as record. Future readers see the warning.

The Human ratifies the amendment WITH the cascade. The amendment is **not ratifiable** until the architect has done the cascade work. Schema-enforced via `required_when: kind == 'amend'` on a `cascade_decisions` field.

This puts the architectural-coherence-of-the-amendment work where it belongs (in the amendment itself), and prevents two failure modes:
- **Auto-stale.** A ratified ruling does not silently become invalid because its cited doctrine §amended; staleness requires explicit architect judgment.
- **Silent staleness.** A ruling cannot stay `ratified` against amended doctrine without the architect having decided what to do about it.

The two principles together — flexible precedent + cascade-on-amendment — mean: doctrine moves easily when evidence justifies it, routine ruling supersession is low-ceremony, and prior rulings under amended doctrine get explicit re-evaluation rather than silent staleness.

## 4. The architect's working surface — `architecture_reviews`

The architect's ruling buffer is the `architecture_reviews` typed store. L171/T077 shipped this as **phase α** of this direction.

### Lifecycle

```
pending → in_review → verdict_issued                         (kind=interpret)
pending → in_review → awaiting_human_ratification → verdict_issued (kind=amend)
                    ↘
                      withdrawn (terminal: architect retracts before verdict)
verdict_issued → superseded (terminal: replaced by later ruling with `supersedes` set)
```

### Inputs (the architect's queue)

The architect pulls from a typed queue, not a push-shaped inbox. Sources:

- **`arch_review_candidate` routing from the gatekeeper** (`intake_items.triaging → routed`). This is phase α's primary input: the router writes an A### row and marks the downstream observation `pending_architecture_review = true` in the same transaction.
- **Cluster-threshold crossings.** When `cluster_key` count crosses the architecture-review threshold (default 3), the next filing into the cluster routes to `arch_review_candidate` regardless of its individual risk flags.
- **Pre-ratification holds** on observation contracts carrying top-level `pending_architecture_review = true`; the architect's verdict gates U1 ratification until a clearing verdict and any required reconciliation are present.
- **Periodic sweeps** after N shipped tasks (drift detection across recent local-fix clusters) — deferred beyond phase α.
- **Self-initiated amend drafts.** The architect notices that doctrine itself needs to move; drafts an amend ruling.
- **Other agents flagging mid-task.** An executor halts and routes "this would amend doctrine" through intake; gatekeeper escalates.

### Outputs

- **Interpret rulings.** Verdict types: `allow_local_fix`, `reframe_contract`, `merge_with_cluster`, `create_primitive_task`, `block_pending_fixes`, `request_human_arch_decision`. Issued by `actor: ai_with_human` and transition `in_review → verdict_issued`.
- **Amendment drafts.** `kind=amend` with `verdict=propose_doctrine_update` and required `cascade_decisions`. `issue-verdict` moves `in_review → awaiting_human_ratification`; `ratify-amend` is pure `actor: human` tier-A token-mediated and rejects `ai_autonomous` / `ai_with_human` even with a valid token.
- **Doctrine-doc updates.** Deferred beyond phase α. There is no doc-diff projection hook in this slice; the ruling row is the durable record.
- **Blocked-or-reframed observation contracts.** Via the `pending_architecture_review` flag.
- **Cluster registry edits** (rename / split / merge) — when L173 ships.

### Composition with local oversight

The Architect oversees **architectural coherence**. plan_reviewer, code_reviewer, codex oversee **local correctness**. Orthogonal axes — they coexist without subsuming each other. The architect can BLOCK U1 ratification on a risky observation contract (pre-ratification gate); it does not interfere with plan or code review once a contract is `ready`. After accept, the architect can also fire post-hoc on shipped clusters (drift detection sweep).

This composition closes the gap that `docs/architecture-coherence.md` names: local-correctness gates (tests, code review, contract ratification) do not catch architectural drift, so a separate trigger surface is required.

## 5. Per-substrate scope

The Architect role is **per-substrate, not global**. Each `.stores/` instance has its own Heart and its own Architect. The doctrine of "Architect interprets, Human amends" is the same shape everywhere; the *content* of the constitution is per-project.

- When stores itself is the substrate (dogfood), the Architect is the architect *of stores*.
- When a client project uses stores, the client has their own Architect overseeing their own Heart.

Stores does not arbitrate client architectural decisions. The substrate-vs-wrapper boundary holds (`docs/philosophy.md` § *What's outside the substrate*).

### The meta escape hatch

Cross-substrate friction is a real channel, not a back-channel. Anyone working in a client repo can file friction *against the stores substrate itself* with `--meta=<stores-path>` (or `STORES_META_PATH=<path>` set once and bare `--meta` on each invocation). The intake lands in stores' gatekeeper; if it routes `arch_review_candidate`, stores' Architect picks it up.

This preserves the substrate boundary while letting client work surface stores-architectural concerns through the documented channel — never through a back-channel into stores' DB. The same authority gates apply: the meta-flag does not relax `--invoker` discipline, and tier-A writes still require token grounding.

## 6. Build sequence: toward typed Heart and typed Architect

Long-term direction broken into ratifiable phases. Each phase is a coherent slice; each can be a substrate task or a small set of tasks. The phases are *direction*, not *commitment* — later phases ratify only when the substrate pulls them forward.

| Phase | What ships | Pulled forward by |
|---|---|---|
| **α** (first slice) | **Shipped in T077/L171.** Dedicated `architecture_reviews` store with A### namespace, `kind ∈ {interpret, amend}`, seven-verdict surface, `cascade_decisions` required for amendments, `pending → in_review → awaiting_human_ratification → verdict_issued` amend path, `pending → in_review → verdict_issued` interpret path, terminal `withdrawn`/`superseded`, flexible-precedent `supersedes`, gatekeeper A### routing, and observation `pending_architecture_review` U1 gate. The T053/L142 tagged-observation stand-in is historical/backfill input only. Architect role still played by Pi via the existing `pi-architect` skill, grounded as `actor: ai_with_human`; there is no typed actor: architect. | Done. |
| **β** | Typed `actor: architect` actor class added to the schema's actor matrix. Authority sits between `ai_with_human` (honor-system) and `human` (cryptographic): autonomous on interpret-rulings, blocked on amend-ratify. `pi-architect` skill grounds writes as `--invoker architect`. | After α validates that the architecture_reviews flow is real load-bearing usage (≥5 ratified rulings, evidence the pi-architect skill is producing them, evidence the gatekeeper is feeding them). Otherwise the typed actor class is premature. |
| **γ** | Typed Heart store (not present in phase α). First sections promoted from prose: probably `philosophy.md § Two-gate operational frame`, `primitives.md` table, `architecture-coherence.md` doctrine §. Citations migrate from path strings to soft-FKs. The .md files become projections of the Heart store, not authoritative copies. | After ≥5 ratified rulings cite the same doc section. That is a real promotion signal: the section is being queried often enough that typed-row queries would help. |
| **δ** | Amend ceremony fully wired. Doc-diff projection hook applies doctrine changes and commits with ruling-id in message. Phase α has no doc-diff projection hook; it only records/ratifies the A### ruling. | After γ. Doc-diff projection requires the Heart store to exist for the diff to write into. |
| **ε** | Architect as auto-fire subscriber on cluster thresholds + pre-ratification holds on risky contracts (not present in phase α). The architect's queue drains autonomously instead of waiting for the orchestrator to dispatch it. | After δ. Subscriber discipline is mature only after the manual flow has been exercised. |
| **ζ** | Curated `cluster_key` registry (L173) + watch dashboards. Promoted cluster keys gain canonical definitions, tunable thresholds, and pointers to the architectural concern they name. | After ε. Cluster-registry curation is meaningful only when the architect is actively pulling clusters. |
| **η** | Pre-ratification Check primitive enforces "risk_class=architecture cannot ratify without architect verdict." Mechanical enforcement of the doctrinal gate. | After ε. Otherwise the Check has nothing to enforce against. |

Phase α has no typed `actor: architect`, no typed Heart store, no doc-diff projection hook, and no auto-fire subscribers. Pi plays Architect through the existing `pi-architect` skill and writes as `actor: ai_with_human` for interpret/draft work; amendment ratification remains `actor: human` tier-A.

Phase α is the first substrate task to ratify against this direction. Subsequent phases surface as observations and ratify individually as the substrate pulls them.

## 7. What this is NOT

Guards against scope creep — what the Architect / Heart layer is explicitly not:

- **Not a process layer above the substrate.** The Architect's writes go through the same CLI as every other actor. There is no privileged Architect channel into the DB.
- **Not a replacement for codex / plan-reviewer / code-reviewer.** Those oversee local correctness; the Architect oversees architectural coherence. Orthogonal axes.
- **Not a deployment system or outer orchestrator.** It is substrate-internal governance.
- **Not a doctrine-as-code framework for client projects.** The shape (Philosophy / Primitives / Architect / Human) may generalize, but stores does not impose its meta-primitive structure onto client substrates. Each substrate has its own Heart.
- **Not a panel or committee yet.** Single-Architect for now; multi-Architect fan-out is open direction (§ 8) but not currently in the build sequence.
- **Not a primitive-discovery mechanism.** New primitives are discovered through realistic-pull on real client work + dogfood pressure (`docs/philosophy.md` § *Pull from real use*). The Architect drafts proposed primitive amendments only after a primitive has surfaced through use; the Architect does not invent primitives top-down.

## 8. Open questions (deferred, named for future direction)

Direction-shaping questions surfaced during intent-hardening but explicitly deferred. Named here so future architectural moves can compose against them or surface fresh tensions:

1. **Multi-architect panels.** When and how to fan out from single Architect to a panel where ≥2 Architect rulings are required for amend recommendations. Surfaces when single-Architect throughput becomes the bottleneck, or when amend-stakes warrant cross-checking. Aesthetic appeal already noted ("I like the idea of many kind of fanning out"); operational pull-forward TBD.
2. **Heart promotion preserves git history.** When prose sections promote into typed Heart rows (phase γ), whether the original git history follows the content into the row or stays with the now-projected file. Out of scope for first slice.
3. **Cross-substrate primitive sharing.** Whether two substrates can share a primitive set (e.g. stores' primitives are reused by a client substrate that imports them rather than redefining). Not explored; client substrates are independent today.
4. **Mapping the meta-primitive structure onto non-stores software.** Whether (Philosophy / Primitives / Architect / Human) generalizes as a constitutional pattern for any sufficiently complex software. Plausible but unverified; deferred until pulled by external use.

## Pointers

- Philosophy: `docs/philosophy.md` (the Heart content as prose, today)
- Primitives: `docs/primitives.md` (the typed constituents, with discovery changelog)
- Architectural-coherence doctrine: `docs/architecture-coherence.md` (the local-correctness-vs-coherence framing that surfaced this layer)
- Risk-and-cluster taxonomy: `docs/risk-and-cluster-taxonomy.md` (typed enums for the gatekeeper)
- Gatekeeper design: `docs/gatekeeper-design.md` (T045 phase 2 — `architecture_reviews` specced as P3 follow-up = L171)
- Engine health: `docs/engine-health.md` (operational measurement — not constitutional)
- Thesis seed: `docs/worklog/2026-05-07/02-heart-constitution-architect-thesis.md`

## Revision history

- **v1.0** (2026-05-07) — initial draft. Two-meta-primitive doctrine (Philosophy + Primitives); three-role authority model (Human / Architect / AI workers); cryptographic moat via interpret vs amend ruling kinds reusing existing tier-A/tier-B actor gates; "software is ever-evolving" elevated to first-order principle with flexible-precedent + cascade-on-amendment as operational consequences; per-substrate scope with `--meta` escape hatch; build sequence α through η. Hardened from the 2026-05-07 thesis through the intent-harden discipline (grill → compress → visualize-skipped → stress-test → realign). Deferred: multi-architect fan-out, Heart-promotion git-history preservation, cross-substrate primitive sharing, mapping the meta-primitive shape onto non-stores software.
