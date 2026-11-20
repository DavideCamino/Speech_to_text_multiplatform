use anyhow::{Result, bail, ensure};

const MAX_LINE_BYTES: usize = 1024 * 1024;
const MAX_EVENT_BYTES: usize = 1024 * 1024;

/// SSE is line based, not network-chunk based. Keep bytes until a complete line
/// arrives so split UTF-8 characters and CRLF boundaries remain intact.
#[derive(Default)]
pub struct Decoder {
    pending: Vec<u8>,
    // Every data line contributes a newline, including an empty value.
    data: String,
    pub done: bool,
}
impl Decoder {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>> {
        let mut output = Vec::new();
        for fragment in bytes.split_inclusive(|&byte| byte == b'\n') {
            if self.done {
                break;
            }
            let line = fragment.strip_suffix(b"\n");
            let length = line.unwrap_or(fragment).len();
            ensure!(
                self.pending.len().saturating_add(length) < MAX_LINE_BYTES,
                "SSE line too large"
            );
            let Some(line) = line else {
                self.pending.extend_from_slice(fragment);
                break;
            };
            if self.pending.is_empty() {
                self.read_line(line, &mut output)?;
            } else {
                let mut pending = std::mem::take(&mut self.pending);
                pending.extend_from_slice(line);
                self.read_line(&pending, &mut output)?;
                pending.clear();
                self.pending = pending;
            }
        }
        Ok(output)
    }

    fn read_line(&mut self, line: &[u8], output: &mut Vec<String>) -> Result<()> {
        let line = std::str::from_utf8(line)?.trim_end_matches('\r');
        if line.is_empty() {
            self.dispatch(output)?;
        } else if let Some(data) = line.strip_prefix("data:") {
            let data = data.strip_prefix(' ').unwrap_or(data);
            // Leave room for the delimiter, even when the data value is empty.
            ensure!(
                self.data.len().saturating_add(data.len()) < MAX_EVENT_BYTES,
                "SSE event too large"
            );
            self.data.push_str(data);
            self.data.push('\n');
        }
        Ok(())
    }

    fn dispatch(&mut self, output: &mut Vec<String>) -> Result<()> {
        // Strip only the final delimiter; the preceding ones join data fields.
        let Some(data) = self.data.strip_suffix('\n') else {
            return Ok(());
        };
        if data == "[DONE]" {
            self.data.clear();
            self.done = true;
            return Ok(());
        }
        let parsed = serde_json::from_str(data);
        self.data.clear();
        let value: serde_json::Value = parsed?;
        // Never surface arbitrary response bodies: they can echo private inputs.
        if value.get("error").is_some() {
            bail!("Server reported a generation error");
        }
        let choices = value.get("choices").and_then(|v| v.as_array());
        ensure!(
            choices.is_some() || value.get("usage").is_some(),
            "Invalid chat stream event"
        );
        if let Some(choice) = choices.and_then(|c| c.first())
            && let Some(text) = choice.pointer("/delta/content").and_then(|v| v.as_str())
        {
            output.push(text.to_owned());
        }
        Ok(())
    }
    pub fn finish(&mut self) -> Result<Vec<String>> {
        let output = self.push(b"\n\n")?;
        ensure!(
            self.done,
            "Connection ended before the generation completed"
        );
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arbitrary_chunk_boundaries() {
        let wire = ": keepalive\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"héllo\"}}]}\r\n\r\ndata: [DONE]\r\n\r\n";
        for size in 1..wire.len() {
            let mut decoder = Decoder::default();
            let mut text = String::new();
            for chunk in wire.as_bytes().chunks(size) {
                text.extend(decoder.push(chunk).unwrap());
            }
            text.extend(decoder.finish().unwrap());
            assert_eq!(text, "héllo");
        }
    }
    #[test]
    fn empty_data_lines_count_toward_the_event_limit() {
        let mut decoder = Decoder::default();
        let lines = b"data:\n".repeat(4096);
        for _ in 0..MAX_EVENT_BYTES / 4096 {
            assert!(decoder.push(&lines).unwrap().is_empty());
        }
        assert_eq!(decoder.data.len(), MAX_EVENT_BYTES);
        let error = decoder.push(b"data:\n").unwrap_err();
        assert_eq!(error.to_string(), "SSE event too large");
        assert_eq!(decoder.data.len(), MAX_EVENT_BYTES);
    }

    #[test]
    fn line_limit_applies_before_buffering_with_or_without_newline() {
        let mut wire = vec![b'x'; MAX_LINE_BYTES];
        wire[0] = b':'; // Even ignored comment lines must respect the limit.
        wire.push(b'\n');
        for suffix in [&wire[..], &wire[..MAX_LINE_BYTES]] {
            let error = Decoder::default().push(suffix).unwrap_err();
            assert_eq!(error.to_string(), "SSE line too large");
        }
        let mut decoder = Decoder::default();
        let split = MAX_LINE_BYTES - 1;
        assert!(decoder.push(&wire[..split]).unwrap().is_empty());
        assert_eq!(
            decoder.push(&wire[split..]).unwrap_err().to_string(),
            "SSE line too large"
        );
        assert_eq!(decoder.pending.len(), split);
    }

    #[test]
    fn multiline_events_join_and_reset_between_dispatches() {
        let mut decoder = Decoder::default();
        let text = decoder.push(b"data:\r\ndata: {\"choices\":\r\ndata: [{\"delta\":{\"content\":\"first\"}}]}\r\n\r\n").unwrap();
        assert_eq!(text, ["first"]);
        assert!(decoder.data.is_empty());
        assert!(
            decoder
                .push(b"data: {\"usage\":{}}\n\n")
                .unwrap()
                .is_empty()
        );
        assert!(decoder.data.is_empty());
        assert!(decoder.push(b"data: [DONE]\n\n").unwrap().is_empty());
        assert!(decoder.done);
    }

    #[test]
    fn truncated_and_invalid_streams_fail() {
        assert!(Decoder::default().finish().is_err());
        assert!(Decoder::default().push(b"data: nope\n\n").is_err());
        assert!(
            Decoder::default()
                .push(b"data: {\"error\":\"secret\"}\n\n")
                .is_err()
        );
    }
}
