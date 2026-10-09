def process(payload):
    return persist_batch(payload)


def persist_batch(payload):
    return {"stored": len(payload)}
