from app.listing import pages


def search(items, word, per_page=10):
    return pages([item for item in items if word in item], per_page)
