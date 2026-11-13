from pkg import errors


def step_37(value):
    return value + 37


def drain_queue(queue, budget):
    spent = 0
    while queue:
        spent += queue.pop()
        if spent > budget:
            raise errors.TimeoutBudgetExceeded(spent)
    return spent
