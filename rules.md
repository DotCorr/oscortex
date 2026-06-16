# OSCortex Engineering Rules — MANDATORY for every agent and contributor

These rules exist because stubbed/incomplete work was shipped or reported as if it
were finished. That wastes the maintainer's time and hides risk. **Read this before
starting any task. Re-read it before reporting a task complete.** No exceptions.

---

## 1. NEVER present a stub as done. Ever.
A stub, mock, placeholder, hardcoded value, fake response, or "happy-path only"
implementation is **NOT** a finished feature.

- If you write one, you **MUST** label it in the code on the line above it:
  `// STUB(<feature>): <what's fake> — real impl needs <what>. Tracked: <where>`
- You **MUST** report it as a stub in your status, using the exact word "stub".
- You may **NEVER** say/write "done", "working", "implemented", "complete", or
  "✅" for anything that contains a stub on its critical path.

## 2. "Done" has one definition: verified working end-to-end.
- Compiles ≠ done. Type-checks ≠ done. Renders a UI ≠ done. Tests pass ≠ done if the
  tests test the stub.
- "Done" means: you ran the **real artifact** (the actual ISO/binary/app the user
  would run — not a proxy, not a unit test of one layer) and **observed the real
  behavior produce the real result**, with proof (screenshot, log, output) saved.
- If you cannot verify it end-to-end, it is **NOT done** — it is "implemented,
  unverified". Say that.

## 3. Report status in one of exactly four states. No fuzzy words.
For every feature/task you touch, classify it as exactly one of:
- **DONE** — implemented AND verified end-to-end on the real artifact (proof exists).
- **UNVERIFIED** — implemented, compiles, but not yet run/proven on the real artifact.
- **SCAFFOLD/STUB** — plumbing or UI exists; core behavior is faked/missing.
- **NOT STARTED**.

Never blur these. "We have a webview" when the engine is a stub = lying by omission.

## 4. Finish the task, or name the gap precisely.
You may not silently narrow scope. If you can't finish:
- State **exactly** what is missing, **why**, and a **realistic effort estimate**.
- Leave a scoped `TODO(<feature>): <missing> — <why> — est <effort>` in the code.
- Do not downgrade a request ("build the browser") into a lesser thing ("build a
  browser-shaped placeholder") and call it the same task.

## 5. Verify on the REAL thing before any "it works" claim.
- Don't test a debug/instrumented build and claim the release build works — build and
  run the **actual release artifact**.
- Don't infer from one layer. The user runs the whole stack; verify the whole stack.
- "Published == verified": nothing ships/goes to cloud unless its exact bytes were run.

## 6. When unsure whether something is complete, assume it is NOT, and check.
Default to skepticism about your own prior work. Open the file. Run it. Look.

## 7. Stubs are allowed ONLY as explicit, tracked stepping stones.
A stub is legitimate to *unblock a pipeline* — but only when (a) labeled per Rule 1,
(b) reported per Rule 3, and (c) the real implementation is tracked as the next step.
A stub is never an acceptable final deliverable for a requested feature.

## 8. Maintain an honest feature ledger.
Keep `docs/FEATURE_STATUS.md` current: every user-facing feature, its state (one of
the four in Rule 3), and the gap if not DONE. Update it whenever you touch a feature.
This is how we stop "I'm pretty sure that's stubbed across the OS" from being true.

---

### Why this is non-negotiable
The maintainer works against time and trusts status reports to plan. A false "done"
breaks that trust and costs more time than the work saved. Honesty about an unfinished
thing is always cheaper than a hidden stub. Slower-but-true beats fast-but-fake.
