# How an issue gets done

The point of this loop: the code can be worked without the user reading all
of it, while every decision that shapes it reaches them. The steps that are
theirs are theirs — never take them on their behalf, never assume one
happened, and never carry on past one without an answer.

## The loop

1. **Plan.** One issue at a time. Write the plan and discuss it before any
   code exists. Implementation starts when the plan is agreed, not before.
2. **Implement.** Inside the agreed plan, work to the end without stopping
   for approval. Changing the plan is a new discussion, not a detail.
3. **Hand it over.** Report what was done, then say what is worth trying in
   the app: things to click and what should happen, not a tour of the diff.
   Stop there.
4. **Run it.** When the user says go, start the app in dev mode
   (`pnpm tauri dev`) and let them drive it. Watch the output and say what
   the app reports; the user says what it does.
5. **Answer what they found.** More instructions means implement them and
   go back to step 3. "Ship" moves on.
6. **Ship, as far as the review.** Commit, then `roborev review`. Nothing is
   pushed and no PR is opened at this step.
7. **Deal with every finding** by category, per
   `.claude/rules/review-findings.md`.
8. **Ask to be tried again.** When no finding is left, stop and ask the user
   to test the feature again. Report what came back and what was done about
   each one. Make no recommendation about whether to review again: that call
   is theirs and the severity of what was found does not decide it.
9. **Their call.** "Re-review" goes back to the review in step 6. "Push and
   merge" runs the rest of `/ship`: push, open the PR, wait for CI, squash
   merge, then clean the branch up.

## Never

- **Never report a feature as working on the strength of tests alone.** Ten
  review rounds, three test suites and a full CI run all passed an app that
  would not start, and the user found it by opening it. Until they have
  driven it, the honest word is "unverified in the app".
- **Never push, open a PR or merge before step 9.** The review in step 6 is
  for us; CI and the PR come after the user has tried the feature.
- **Never move to the next issue** without agreeing which one it is.
