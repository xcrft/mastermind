use super::{truncate_text, xml_attribute, xml_unescape, SourceFailure, MAX_SOURCE_FACTS};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::time::Instant;

pub(super) struct ParsedJunit {
    pub cases: Vec<JunitCase>,
    pub facts_total: usize,
    pub partial: bool,
    pub invalid_records: bool,
    pub work_limited: bool,
    pub deadline_reached: bool,
}

pub(super) struct JunitCase {
    pub file: Option<String>,
    pub name: String,
    pub class_name: Option<String>,
    pub status: JunitStatus,
    pub message: String,
    pub duration_ms: u64,
}

#[derive(Clone, Copy)]
pub(super) enum JunitStatus {
    Passed,
    Failed,
    Error,
    Skipped,
}

struct CaseBuilder {
    file: Option<String>,
    name: String,
    class_name: Option<String>,
    status: JunitStatus,
    message: String,
    duration_ms: u64,
    reading_detail: bool,
}

impl CaseBuilder {
    fn from_event(event: &quick_xml::events::BytesStart<'_>) -> Self {
        Self {
            file: xml_attribute(event, b"file"),
            name: xml_attribute(event, b"name").unwrap_or_else(|| "unnamed testcase".into()),
            class_name: xml_attribute(event, b"classname"),
            status: JunitStatus::Passed,
            message: String::new(),
            duration_ms: xml_attribute(event, b"time")
                .and_then(|value| value.parse::<f64>().ok())
                .filter(|value| value.is_finite() && *value >= 0.0)
                .map(|seconds| (seconds * 1_000.0).round().min(u64::MAX as f64) as u64)
                .unwrap_or(0),
            reading_detail: false,
        }
    }

    fn start_detail(&mut self, status: JunitStatus, event: &quick_xml::events::BytesStart<'_>) {
        self.status = status;
        self.message = xml_attribute(event, b"message").unwrap_or_default();
        self.reading_detail = true;
    }

    fn append_detail(&mut self, detail: &str) {
        if self.message.is_empty() {
            self.message = detail.to_string();
        } else if !detail.is_empty() {
            self.message.push_str(" · ");
            self.message.push_str(detail);
        }
        self.message = truncate_text(&self.message, 500);
    }

    fn finish(self) -> JunitCase {
        JunitCase {
            file: self.file,
            name: truncate_text(&self.name, 200),
            class_name: self.class_name.map(|value| truncate_text(&value, 240)),
            status: self.status,
            message: truncate_text(&self.message, 500),
            duration_ms: self.duration_ms,
        }
    }
}

pub(super) fn parse(bytes: &[u8], deadline: Option<Instant>) -> Result<ParsedJunit, SourceFailure> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut cases = Vec::new();
    let mut current = None;
    let mut facts_total = 0usize;
    let mut partial = false;
    let mut invalid_records = false;
    let mut work_limited = false;
    let mut deadline_reached = false;
    let mut saw_suite = false;
    let mut events = 0usize;

    loop {
        if events.is_multiple_of(4_096) && deadline.is_some_and(|value| Instant::now() >= value) {
            partial = true;
            deadline_reached = true;
            break;
        }
        events += 1;
        match reader
            .read_event_into(&mut buffer)
            .map_err(|_| SourceFailure::InvalidFormat)?
        {
            Event::Start(event) => match event.local_name().as_ref() {
                b"testsuites" | b"testsuite" => saw_suite = true,
                b"testcase" => {
                    saw_suite = true;
                    if facts_total >= MAX_SOURCE_FACTS {
                        partial = true;
                        work_limited = true;
                        break;
                    }
                    facts_total += 1;
                    current = Some(CaseBuilder::from_event(&event));
                }
                b"failure" => {
                    if let Some(case) = current.as_mut() {
                        case.start_detail(JunitStatus::Failed, &event);
                    }
                }
                b"error" => {
                    if let Some(case) = current.as_mut() {
                        case.start_detail(JunitStatus::Error, &event);
                    }
                }
                b"skipped" => {
                    if let Some(case) = current.as_mut() {
                        case.start_detail(JunitStatus::Skipped, &event);
                    }
                }
                _ => {}
            },
            Event::Empty(event) => match event.local_name().as_ref() {
                b"testsuites" | b"testsuite" => saw_suite = true,
                b"testcase" => {
                    saw_suite = true;
                    if facts_total >= MAX_SOURCE_FACTS {
                        partial = true;
                        work_limited = true;
                        break;
                    }
                    facts_total += 1;
                    cases.push(CaseBuilder::from_event(&event).finish());
                }
                b"failure" => {
                    if let Some(case) = current.as_mut() {
                        case.start_detail(JunitStatus::Failed, &event);
                        case.reading_detail = false;
                    }
                }
                b"error" => {
                    if let Some(case) = current.as_mut() {
                        case.start_detail(JunitStatus::Error, &event);
                        case.reading_detail = false;
                    }
                }
                b"skipped" => {
                    if let Some(case) = current.as_mut() {
                        case.start_detail(JunitStatus::Skipped, &event);
                        case.reading_detail = false;
                    }
                }
                _ => {}
            },
            Event::Text(event) => {
                if let Some(case) = current.as_mut().filter(|case| case.reading_detail) {
                    let text = std::str::from_utf8(event.as_ref())
                        .map_err(|_| SourceFailure::InvalidUtf8)?;
                    let text = xml_unescape(text)?;
                    case.append_detail(&text);
                }
            }
            Event::CData(event) => {
                if let Some(case) = current.as_mut().filter(|case| case.reading_detail) {
                    let text = std::str::from_utf8(event.as_ref())
                        .map_err(|_| SourceFailure::InvalidUtf8)?;
                    case.append_detail(text);
                }
            }
            Event::End(event) => match event.local_name().as_ref() {
                b"testcase" => {
                    if let Some(case) = current.take() {
                        cases.push(case.finish());
                    } else {
                        partial = true;
                        invalid_records = true;
                    }
                }
                b"failure" | b"error" | b"skipped" => {
                    if let Some(case) = current.as_mut() {
                        case.reading_detail = false;
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if current.is_some() {
        partial = true;
        invalid_records = true;
    }
    if !saw_suite {
        return Err(SourceFailure::InvalidFormat);
    }
    Ok(ParsedJunit {
        cases,
        facts_total,
        partial,
        invalid_records,
        work_limited,
        deadline_reached,
    })
}
