# Rust in this repository

Invariants the review has caught being broken more than once. Walk these
before asking for a review, not after.

## Nothing succeeds quietly

- **Never ignore a value that says whether the work happened.** A `bool` from
  a tag write, a `Result` from a file write, the rows a statement touched. No
  `let _ =` on anything that changes state.
- **A change that matched nothing is an error.** Renaming, deleting or adding
  to something that is not there must say so; returning `Ok` makes a button
  that did nothing look like a button that worked.
- **A value the caller cannot use is refused, not reinterpreted.** An
  out-of-range rating is an error, never silently treated as "no rating": that
  erases what the file held.

## Commands are a public surface

- **Validate at the boundary.** A command is reachable from anything that can
  talk to the frontend, so the ranges the interface offers are not a
  guarantee. Check them where the command starts.
- **Commands run on a thread pool.** Anything they read, change and write back
  needs a lock held across all three, or two of them lose each other's work.

## The file decides

- **Write the file first, then the cache.** If the file write fails, the cache
  must not be touched: the music carries the truth, and the cache is thrown
  away and rebuilt.
- **Refuse to write over what cannot be read.** A user-data file that is
  absent may start from the defaults. One that exists but cannot be parsed is
  an error: writing defaults over it destroys what nothing can rebuild, such
  as the playlists.

## Shape

- **Pull the decision out of the command.** A command taking Tauri `State`
  cannot be tested; the rule it applies can, once it is a plain function
  taking plain values. Do it while writing it, not when the review asks.
