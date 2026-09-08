# Roborev loop

How the review loop terminates in this repository. Overrides the interactive
default in `claude/rules/roborev-review-handling.md`: reviews here run in auto
mode, without per-finding approval.

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

## Why

Every fix is new code, and new code produces new findings. Looping until a
review is empty never terminates. Gating the loop on severity keeps the
important findings addressed while letting the work ship.
