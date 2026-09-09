# What to do with a review finding

Findings are not one kind of thing, and answering them as though they were
is what produced ten rounds of the same mistakes. Sort each one into a
category and do what that category says.

The list is incomplete on purpose. **When a finding fits none of these, ask
rather than guess**, and write the answer here so the next one is covered.

## Categories

### The fix is in the tests only

Autonomous — write it. Then **break the code the test covers and watch the
test fail for the right reason**, and put the code back. A test nobody has
seen fail is not evidence that anything is covered; twice in this repository
a test reported coverage that did not exist. Report it as done, saying what
was broken and how the test complained.

This covers a test that is missing and a test whose assertion is too weak to
fail. It stops where production code starts.

### A fix would change code

Never write it first. Explain what was found with a code example — the lines
as they stand and what the fix would make of them — and work it out with the
user. This holds for a one-line fix, for a finding whose suggested fix looks
obviously right, and for a judgement call about cost that has no right
answer. Deciding it alone is the thing this rule exists to stop.

### The finding is wrong

Try to disprove it rather than to argue with it: mutate the code, run the
test, drive the UI. Reviewers have claimed a failure mode that could not be
reached and offered a fix that would have stopped the app booting, and both
took minutes to settle empirically. If the evidence disproves it, dismiss it
and say so in the report at the end, with what was run and what happened. If
it cannot be settled that way, treat it as a code-change finding and bring
it to the user.

## Prefer a check over a rule

When a finding names something a lint, a type or a test could catch for
good, add that instead of prose. A rule in this directory has to be
remembered; `clippy::assertions_on_result_states` found four bad assertions
at once and fails the build, one commit after the prose version of the same
rule was written and broken.

## Mechanics

- **Never block on a review.** Launch the wait in a subagent and carry on;
  say what is running so it does not look like silence.
- **The subagent triages and never edits.** It waits, reads each finding
  against the code, gathers the evidence above, and reports. Fixing happens
  in the session.
- **The first review of a branch covers the branch; later rounds review only
  the commit that answered the last one.** Re-reading the whole branch each
  round keeps finding new things in old code.
- **A finding already dealt with does not come back.** When a whole-branch
  review re-surfaces one that was fixed or dismissed in an earlier round,
  say so and move on rather than reopening it.
- **Walk the checklists first.** The `/elm`, `/rust` and `/tauri` skills end
  in a checklist and a table of anti-patterns. A review is not the place to
  discover them.
