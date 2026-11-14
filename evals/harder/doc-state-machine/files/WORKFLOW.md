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
- `order.apply("tick")` is allowed in every state that is not final. It never changes the
  state and is not added to the history.
- From `delivered`, `return` is allowed only while at most 3 ticks have happened since
  the order was delivered: after 0, 1, 2 or 3 ticks it succeeds, after a fourth it raises
  `ValueError`. Ticks from before the delivery do not count.
- From `shipped`, `return` is always allowed, however many ticks have happened.
- `order.can(event)` returns whether `apply(event)` would succeed, without changing
  anything.
