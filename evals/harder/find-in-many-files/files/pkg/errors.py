class WorkError(Exception):
    pass


class TimeoutBudgetExceeded(WorkError):
    pass
