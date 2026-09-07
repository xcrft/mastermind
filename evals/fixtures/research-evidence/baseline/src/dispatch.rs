type Handler = fn(&str) -> usize;

pub fn payload_size(payload: &str) -> usize {
    payload.len()
}

pub fn install_handlers() -> [Handler; 1] {
    [payload_size]
}

pub fn dispatch(handler: Handler, payload: &str) -> usize {
    handler(payload)
}
