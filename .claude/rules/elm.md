# Elm in this repository

Invariants the review has caught being broken more than once. Walk these
before asking for a review, not after.

## Rendering

- **`Html.Lazy` arguments are model fields or primitives.** Elm compares them
  by reference, so anything built while rendering (`Maybe.map`, `List.sortBy`,
  a record literal, a lambda) never matches and the memo silently does
  nothing. Pass a plain `Int` where a `Maybe Int` is tempting, and rebuild the
  `Maybe` inside the lazy function.
- **Derived collections live in the model.** Sorting, filtering and grouping
  happen in `update` and are stored; a view that derives them hands a new list
  to every render. The player reports its position several times a second, so
  a view that recomputes is a view that recomputes five times a second.
- **Adding a lazy node is not the fix on its own.** After adding one, check
  every argument against the two rules above, and check the whole file for the
  same mistake rather than the one instance the review named.

## Events

- **Read values out of the event, never out of the model captured at render
  time.** `Html.Events.targetValue` and the event's own `detail` are current;
  a value closed over in the view is one frame stale, and the browser sends
  several events between two frames.
- **A control that replaces another takes focus.** Swapping a button for an
  input leaves focus on `body`, so the field cannot be typed into and its
  `blur` handler can never fire. Focus it with `Browser.Dom.focus`.

## Commands and replies

- **Exactly one handler owns a command name.** A reply no handler claims is
  dropped, and its failure never reaches the interface.
- **Every command's `Err` branch shows something.** A command whose failure is
  swallowed looks to the user like a button that does nothing.
- **A reply only reloads what it changed.** Reloading a panel that does not
  show the changed value rebuilds it for nothing.

## State

- **Clearing a selection also clears what was derived from it.** Dropping the
  open playlist without dropping its rows shows those rows under whichever tab
  takes over, until the reload lands.
