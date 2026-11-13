from app.paging import page_count, page_slice


def pages(items, per_page=10):
    return [page_slice(items, n, per_page) for n in range(1, page_count(len(items), per_page) + 1)]
