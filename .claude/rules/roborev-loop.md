# Roborev loop

How the review loop runs and terminates in this repository. Overrides the
interactive default in `claude/rules/roborev-review-handling.md`: reviews here
run in auto mode, without per-finding approval.

## Rules

- **Auto mode.** Verify each finding, then fix what is worth fixing. Low
  findings may be fixed too; nothing is off limits.
- **Re-review only above low.** Trigger another review round only when the
  last one reported at least one finding above `Low`. A round that comes back
  with nothing but `Low` ends the loop.
- **After the last round**, fix what is worth fixing from the remaining low
  findings without reviewing again, then commit, push, wait for CI and merge.
- **Say what was left.** Report the low findings that were not acted on, so
  the decision is visible rather than silent.

## Before asking for a review

- **Walk the checklists first.** The `/elm`, `/rust` and `/tauri` skills end
  in a checklist and a table of anti-patterns; `.claude/rules/testing.md`
  exists because the review found the same mistakes round after round. A
  review is not the place to discover any of them.
- **Fix the class, not the instance.** When a finding names one broken lazy
  argument, one ignored write result or one unvalidated range, search for the
  others before answering it. Four rounds went on one Elm invariant fixed one
  instance at a time.
- **Never defer the same finding twice.** A low finding that comes back has
  stopped being cheap to ignore: fix it, or file it as an issue and say so.
  Duplicated stylesheet blocks were reported in three separate rounds and
  skipped each time.

## Waiting

- **Never block on a review.** Launch the wait in a subagent and carry on with
  something else; the notification arrives when it finishes. Blocking on the
  result parks the whole session for the several minutes a review takes.
- **The subagent triages.** It waits for the review, reads each finding against
  the code, and reports which claims hold, which are wrong and why, and which
  need a decision. It never edits anything: fixing stays here.
- **Say what is running** before moving on, so a review in flight is visible
  rather than looking like silence.

## Scope

- **The first review of a branch covers the branch** (`roborev review --branch`).
- **Every later round reviews only the commit that answered the previous
  round** (`roborev review` on `HEAD`). What was already reviewed stays
  reviewed; re-reading the whole branch each time keeps finding new things in
  old code and the loop never converges.

## Why

Every fix is new code, and new code produces new findings. Looping until a
review is empty never terminates, and re-reviewing the whole branch every
round makes that worse. Gating the loop on severity, and narrowing later
rounds to what actually changed, keeps the important findings addressed while
letting the work ship.
