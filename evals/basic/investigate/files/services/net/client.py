from services.net import config


def attempts():
    return config.retry_limit + 1
