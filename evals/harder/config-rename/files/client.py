import config


def describe(path=None):
    settings = config.load(path)
    return f"{settings['host']}: wait {settings['timeout']}s, retry {settings['retries']}x"


def deadline_seconds(path=None):
    return config.load(path)["timeout"] * (config.load(path)["retries"] + 1)
