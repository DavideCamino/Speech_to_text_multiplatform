//! UI-independent transcript data and lossless text/subtitle serialization.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub start_ms: u64,
    pub end_ms: u64,
    /// Original recognition text, including boundary whitespace. Adjacent
    /// fragments are concatenated verbatim; callers must not trim each one.
    pub text: String,
}

#[derive(Clone, Debug, Default)]
pub struct Transcript {
    pub segments: Vec<Segment>,
}

impl Transcript {
    pub fn text(&self) -> String {
        self.segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<String>()
            .trim()
            .to_owned()
    }

    pub fn end_ms(&self) -> u64 {
        self.segments.last().map_or(0, |s| s.end_ms)
    }

    pub fn append(&mut self, segments: Vec<Segment>) {
        for mut segment in segments {
            segment.start_ms = segment.start_ms.max(self.end_ms());
            if !segment.text.trim().is_empty() && segment.end_ms > segment.start_ms {
                self.segments.push(segment);
            }
        }
    }

    pub fn srt(&self) -> String {
        self.segments
            .iter()
            .enumerate()
            .map(|(i, s)| {
                format!(
                    "{}\n{} --> {}\n{}\n\n",
                    i + 1,
                    timestamp(s.start_ms),
                    timestamp(s.end_ms),
                    s.text.trim()
                )
            })
            .collect()
    }
}

fn timestamp(ms: u64) -> String {
    format!(
        "{:02}:{:02}:{:02},{:03}",
        ms / 3_600_000,
        (ms / 60_000) % 60,
        (ms / 1000) % 60,
        ms % 1000
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn concatenates_raw_fragments_without_inventing_spaces() {
        for (fragments, expected) in [
            (vec![" Hello", ",", " world", "!"], "Hello, world!"),
            (vec![" trans", "crip", "tion"], "transcription"),
            (vec!["你", "好", "。"], "你好。"),
            (vec!["こん", "にちは", "。"], "こんにちは。"),
            (vec![" Hello ", "world", "! "], "Hello world!"),
        ] {
            let mut transcript = Transcript::default();
            for (index, text) in fragments.iter().enumerate() {
                transcript.append(vec![Segment {
                    start_ms: index as u64 * 100,
                    end_ms: index as u64 * 100 + 100,
                    text: (*text).into(),
                }]);
            }
            assert_eq!(transcript.text(), expected);
            assert_eq!(
                transcript
                    .segments
                    .iter()
                    .map(|s| s.text.as_str())
                    .collect::<Vec<_>>(),
                fragments
            );
        }
    }

    #[test]
    fn srt_and_unicode() {
        let mut t = Transcript::default();
        t.append(vec![Segment {
            start_ms: 3_661_002,
            end_ms: 3_663_040,
            text: " Caffè ☕ ".into(),
        }]);
        assert_eq!(t.text(), "Caffè ☕");
        assert_eq!(t.srt(), "1\n01:01:01,002 --> 01:01:03,040\nCaffè ☕\n\n");
    }
    #[test]
    fn normalizes_overlaps_and_rejects_empty_cues() {
        let mut t = Transcript::default();
        t.append(vec![
            Segment {
                start_ms: 0,
                end_ms: 100,
                text: "first".into(),
            },
            Segment {
                start_ms: 90,
                end_ms: 200,
                text: " second".into(),
            },
            Segment {
                start_ms: 200,
                end_ms: 200,
                text: "invalid".into(),
            },
        ]);
        assert_eq!(t.segments.len(), 2);
        assert_eq!(t.segments[1].start_ms, 100);
        assert_eq!(t.text(), "first second");
    }
}
