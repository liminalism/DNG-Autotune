# Agent protocol

## Project knowledge (AKR)

Durable project knowledge lives in `.akr/` as typed records, not in Markdown.
`docs/generated/` is build output. Follow this protocol.

**Before starting any task**
1. `knowledge.context --goal <milestone|work|track>` for the thing you are working on.
   Add `--paths` for the files you expect to touch.
2. Read the bundle in full. Contradictions and staleness warnings are always included
   and are never noise.

**While working**
- Look things up with `knowledge.get`; find them with `knowledge.search`.
  Search ranks results; it never grants authority. A record's standing comes from its
  state, its scope, and its relations.
- Scratch notes go in `.agent/scratch/`. Nobody reviews them and nothing depends on them.

**When something becomes durable**
- New knowledge: `knowledge.propose`. Observations need `observed_at` and, if they can
  go out of date, `watches`.
- Changed knowledge: `knowledge.revise`. Never edit a `.akr` file directly, and never
  edit a record that is not `proposed`.
- Replacing a plan: `knowledge.supersede`, with a disposition for every unfinished
  child. The tool will list them; answer each one.
- Finishing work: record what you observed with `knowledge.evidence_add`, then
  `knowledge.complete` with evidence for every acceptance check. Evidence records
  state what was observed; they never state what they verify.
- Unsure what a kind requires? `akr explain <kind>` prints its schema.

**After any code change that satisfies, changes, or retires a `work` record's intent**
- In the same working-tree change as the code, `akr revise` the work record (`proposed`→`active`→`completed`/`abandoned` as appropriate). Don't defer the ledger to "later" — code without a record is invisible to `docs/generated/` and to review.
- Add `akr evidence add` for what you observed (command + artifact + summary, `result pass` only when the observed output actually matches). Then, if the record has acceptance checks, `akr complete --check <id>=@evidence/n`.
- Run `akr build` and `akr check` (equivalently `knowledge.validate`) before handoff. If the build reports `akr.lock is now stale`, rebuilding is mandatory — `akr.lock` and `docs/generated/` are build output, never hand-edited.

**Git ↔ AKR lockstep**
- One logical change = one scope: `src/*` + `.akr/records/*` + regenerated `docs/generated/` + `akr.lock` travel together. Never commit code that implements a slice without its `akr revise`/`evidence` in the same commit, and never mark a record `completed` without the code present in the same tree. CI's `akr check` + `akr build --check` will reject either half.
- Treat `git status` and `akr check` as paired gates before handoff or commit: if the code is dirty, the ledger must be dirty in the same direction, and vice-versa. If a commit is made (or should be), the ledger entry is part of that commit — not a follow-up.

**Papercuts**
- When you hit a small friction while working — a tool call that missed and had to be
  retried, a confusing or undocumented setup step, a flaky command, a stale cache, a
  misleading error, a non-obvious gotcha — log it with `knowledge.papercut` (or
  `akr papercut -m <agent> "message"`). One or two sentences: what you were doing,
  what got in the way (a guess at the cause/fix is a bonus). Do this proactively, in
  the moment, even though none of these are blocking — logged together they show where
  the project needs sanding down. This is distinct from durable records (knowledge) and
  from `.agent/scratch/` (working notes).

**Never**
- Never edit `docs/generated/` — it is regenerated and CI checks it.
- Never read `.akr/cache/` — it is a private cache.
- Never delete a record. Move it to a terminal state instead.
- Never hand-edit `.akr/akr.lock`.

**Before handing back**
- `akr build` then `akr check` (or `knowledge.validate`). If it reports diagnostics or `akr.lock is now stale`, fix them or say so explicitly. Include both `git status` and `akr check` output in the handoff.
