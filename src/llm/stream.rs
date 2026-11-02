use anyhow::{Result, bail, ensure};

/// SSE is line based, not network-chunk based. Keep bytes until a complete line
/// arrives so split UTF-8 characters and CRLF boundaries remain intact.
#[derive(Default)]
pub struct Decoder {
    pending: Vec<u8>,
    data: Vec<String>,
    pub done: bool,
}
impl Decoder {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>> {
        self.pending.extend_from_slice(bytes);
        let mut output = Vec::new();
        while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
            let line: Vec<_> = self.pending.drain(..=end).collect();
            let line = std::str::from_utf8(&line)?.trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                self.dispatch(&mut output)?;
            } else if let Some(data) = line.strip_prefix("data:") {
                self.data
                    .push(data.strip_prefix(' ').unwrap_or(data).to_owned());
            }
            ensure!(
                self.data.iter().map(String::len).sum::<usize>() < 1024 * 1024,
                "SSE event too large"
            );
        }
        ensure!(self.pending.len() < 1024 * 1024, "SSE line too large");
        Ok(output)
    }
    fn dispatch(&mut self, output: &mut Vec<String>) -> Result<()> {
        if self.data.is_empty() || self.done {
            return Ok(());
        }
        let data = std::mem::take(&mut self.data).join("\n");
        if data == "[DONE]" {
            self.done = true;
            return Ok(());
        }
        let value: serde_json::Value = serde_json::from_str(&data)?;
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
