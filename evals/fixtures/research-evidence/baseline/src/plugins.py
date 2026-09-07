PLUGIN_TARGETS = {"scrub": "scrub_payload"}


def scrub_payload(data):
    return data.strip()


def invoke_plugin(name, data):
    callback = globals()[PLUGIN_TARGETS[name]]
    return callback(data)
