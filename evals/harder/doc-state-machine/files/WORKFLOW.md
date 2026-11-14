# Order workflow

`Order()` starts in state `new`. `order.state` is the current state and
`order.history` the list of states it has been in, oldest first, starting with `new`.
`order.apply(event)` moves the order and returns the new state.

| From      | Event     | To        |
|-----------|-----------|-----------|
| new       | pay       | paid      |
| new       | cancel    | cancelled |
| paid      | ship      | shipped   |
| paid      | refund    | refunded  |
| paid      | cancel    | refunded  |
| shipped   | deliver   | delivered |
| shipped   | return    | returned  |
| delivered | return    | returned  |
| returned  | refund    | refunded  |

Rules:

- An event that is not in the table for the current state raises `ValueError` whose
  message contains both the state and the event, and changes nothing.
- `cancelled` and `refunded` are final: every event raises `ValueError` there.
- `return` is only allowed within 3 `tick` events after `deliver`. `order.apply("tick")`
  is allowed in every state that is not final, never changes the state, and is not
  added to the history.
- `order.can(event)` returns whether `apply(event)` would succeed, without changing
  anything.
