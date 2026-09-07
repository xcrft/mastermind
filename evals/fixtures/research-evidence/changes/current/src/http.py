def process(payload):
    return decode_request(payload)


def decode_request(payload):
    return {"request": payload}
