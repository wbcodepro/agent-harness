def count_pairs(values, target):
    """Number of pairs (i, j), i < j, with values[i] + values[j] == target."""
    count = 0
    for i in range(len(values)):
        for j in range(i + 1, len(values)):
            if values[i] + values[j] == target:
                count += 1
    return count
