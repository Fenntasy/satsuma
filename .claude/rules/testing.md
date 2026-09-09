# Tests in this repository

Tests here have twice reported coverage that did not exist. These rules are
about that, not about how much to test.

## A test must be able to fail

- **Never accept either outcome.** A test written as "if it succeeded assert
  this, if it failed assert that" passes whatever the code does and tells you
  nothing. Decide which is right, assert it alone, and if the answer is not
  known yet, run the code and find out.
- **Prove the path under test is reached.** A test named for a failed write
  that fails on the read before it never sees the write. Assert the earlier
  steps succeed, so the failure comes from the step being tested.
- **Assert the reason, not just the failure.** `is_err()` passes for the wrong
  error. Check what it says.
- **Watch for assertions that hold trivially.** A guard tested with input that
  terminates on its own without the guard, or a threshold so loose that the
  broken behaviour passes, is a test of nothing.

## Assert what someone else sees

- **A round trip through our own code proves little.** That a rating we wrote
  can be read back by us does not mean another player can read it. Assert the
  field on disk, the command sent, or what the interface shows.
- **The layer where Elm, the bridge and the command names meet has its own
  tests** in `tests/e2e`. A command renamed on one side passes every unit test
  on both.

## When a fix lands

- **Test the class, not the instance.** A fix for one broken lazy argument, one
  ignored write result or one unvalidated range gets a test that would also
  catch the next one of its kind.
