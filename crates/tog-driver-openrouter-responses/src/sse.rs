use std::collections::VecDeque;
use std::fmt::{Display, Formatter};

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum SseError {
    InvalidUtf8(String),
}

impl Display for SseError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUtf8(message) => write!(
                formatter,
                "server-sent event data was not valid UTF-8: {message}"
            ),
        }
    }
}

impl std::error::Error for SseError {}

#[derive(Debug)]
pub(crate) struct ServerSentEvent {
    pub(crate) name: Option<String>,
    pub(crate) data: String,
}

#[derive(Default)]
pub(crate) struct ServerSentEventDecoder {
    bytes: Vec<u8>,
    event_name: Option<String>,
    data_lines: Vec<String>,
    events: VecDeque<ServerSentEvent>,
}

impl ServerSentEventDecoder {
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Result<(), SseError> {
        self.bytes.extend_from_slice(bytes);
        while let Some(newline_position) = self.bytes.iter().position(|byte| *byte == b'\n') {
            let mut line = self.bytes.drain(..=newline_position).collect::<Vec<_>>();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            self.process_line(line)?;
        }
        Ok(())
    }

    pub(crate) fn finish(&mut self) -> Result<(), SseError> {
        if !self.bytes.is_empty() {
            let mut line = std::mem::take(&mut self.bytes);
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            self.process_line(line)?;
        }
        self.dispatch_event();
        Ok(())
    }

    pub(crate) fn next(&mut self) -> Option<ServerSentEvent> {
        self.events.pop_front()
    }

    pub(crate) fn clear(&mut self) {
        self.events.clear();
        self.data_lines.clear();
        self.event_name = None;
    }

    fn process_line(&mut self, line: Vec<u8>) -> Result<(), SseError> {
        let line =
            String::from_utf8(line).map_err(|error| SseError::InvalidUtf8(error.to_string()))?;
        if line.is_empty() {
            self.dispatch_event();
            return Ok(());
        }
        if line.starts_with(':') {
            return Ok(());
        }

        let (field, value) = line
            .split_once(':')
            .map_or((line.as_str(), ""), |(field, value)| {
                (field, value.strip_prefix(' ').unwrap_or(value))
            });
        match field {
            "event" => self.event_name = Some(value.to_owned()),
            "data" => self.data_lines.push(value.to_owned()),
            _ => {}
        }
        Ok(())
    }

    fn dispatch_event(&mut self) {
        if !self.data_lines.is_empty() {
            self.events.push_back(ServerSentEvent {
                name: self.event_name.take(),
                data: self.data_lines.join("\n"),
            });
            self.data_lines.clear();
        } else {
            self.event_name = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ServerSentEventDecoder, SseError};

    fn collect(input: &[&[u8]]) -> Vec<(Option<String>, String)> {
        let mut decoder = ServerSentEventDecoder::default();
        for chunk in input {
            decoder
                .push(chunk)
                .expect("the decoder should accept the chunk");
        }
        decoder.finish().expect("the decoder should finish");
        let mut events = Vec::new();
        while let Some(event) = decoder.next() {
            events.push((event.name, event.data));
        }
        events
    }

    #[test]
    fn framing_handles_both_line_endings_and_event_names() {
        let events = collect(&[br#"event: message
data: one

event: message
data: two

"#]);

        assert_eq!(
            events,
            vec![
                (Some("message".to_owned()), "one".to_owned()),
                (Some("message".to_owned()), "two".to_owned()),
            ]
        );
    }

    #[test]
    fn multi_line_data_is_joined_and_comments_are_ignored() {
        let events = collect(&[b": keep going\ndata: first\ndata: second\n\n".as_slice()]);

        assert_eq!(events, vec![(None, "first\nsecond".to_owned())]);
    }

    #[test]
    fn utf8_characters_split_across_chunks_stay_whole() {
        let greeting = "data: héllo ☃\n\n";
        let middle = greeting.len() / 2;
        let events = collect(&[
            greeting.as_bytes()[..middle].as_ref(),
            greeting.as_bytes()[middle..].as_ref(),
        ]);

        assert_eq!(events, vec![(None, "héllo ☃".to_owned())]);
    }

    #[test]
    fn crlf_line_endings_are_stripped() {
        let events = collect(&[b"data: hello\r\n\r\n".as_slice()]);

        assert_eq!(events, vec![(None, "hello".to_owned())]);
    }

    #[test]
    fn a_trailing_event_without_a_blank_line_is_flushed_by_finish() {
        let events = collect(&[b"data: hello\n".as_slice()]);

        assert_eq!(events, vec![(None, "hello".to_owned())]);
    }

    #[test]
    fn invalid_utf8_is_a_framing_error() {
        let mut decoder = ServerSentEventDecoder::default();
        let error = decoder
            .push(b"data: \xff\xfe\n\n")
            .expect_err("invalid UTF-8 should be rejected");

        assert!(matches!(error, SseError::InvalidUtf8(_)));
    }

    #[test]
    fn clear_drops_buffered_events() {
        let mut decoder = ServerSentEventDecoder::default();
        decoder
            .push(b"data: hello\n\n".as_slice())
            .expect("the decoder should accept the chunk");
        decoder.clear();

        assert!(decoder.next().is_none());
    }
}
