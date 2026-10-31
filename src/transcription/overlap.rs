//! Reconcile word-timed hypotheses before publishing irreversible text.
use crate::transcript::Segment;

pub(super) const TAIL_MS: u64 = 2_000;
pub(super) const OVERLAP_MS: u64 = 4_000;
const TIMESTAMP_TOLERANCE_MS: u64 = 1_500;

#[derive(Default)]
pub(super) struct Reconciler {
    // Published words are retained only as alignment anchors, never re-emitted.
    context: Vec<Segment>,
    pending: Vec<Segment>,
    published_end: u64,
    previous_end: Option<u64>,
}

impl Reconciler {
    pub fn update(
        &mut self,
        words: Vec<Segment>,
        start: u64,
        end: u64,
        last: bool,
    ) -> (Vec<Segment>, String) {
        let mut old = self.context.clone();
        let published = old.len();
        old.append(&mut self.pending);
        let mut merged = if let Some(previous_end) = self.previous_end {
            // A monotonic, time-constrained word alignment avoids mistaking a
            // genuinely repeated phrase elsewhere for duplicated overlap text.
            let anchors = align(&old, &words);
            let anchor = anchors.into_iter().rev().find(|&(i, _)| {
                i + 1 >= published && old[i].end_ms <= previous_end.saturating_sub(300)
            });
            if let Some((i, j)) = anchor {
                let mut merged = old[published..=i].to_vec();
                merged.extend_from_slice(&words[j + 1..]);
                merged
            } else {
                // Recognition may disagree completely or return silence. Keep
                // the earlier hypothesis on its side of a WORD-level seam;
                // never throw away a whole sentence because of its midpoint.
                let seam = self
                    .published_end
                    .max(start + previous_end.saturating_sub(start) / 2);
                let mut merged: Vec<_> = old
                    .into_iter()
                    .skip(published)
                    .filter(|word| midpoint(word) < seam)
                    .collect();
                merged.extend(words.into_iter().filter(|word| midpoint(word) >= seam));
                merged
            }
        } else {
            words
        };
        // Whisper timestamps can overlap or drift. Keep text, but make cues
        // monotonic rather than letting Transcript::append silently drop it.
        let mut cursor = self.published_end;
        for word in &mut merged {
            word.start_ms = word.start_ms.max(cursor);
            word.end_ms = word.end_ms.max(word.start_ms + 1);
            cursor = word.end_ms;
        }
        let cutoff = if last {
            u64::MAX
        } else {
            end.saturating_sub(TAIL_MS)
        };
        let count = merged
            .iter()
            .take_while(|word| word.end_ms <= cutoff)
            .count();
        self.pending = merged.split_off(count);
        if let Some(word) = merged.last() {
            self.published_end = word.end_ms;
        }
        self.context.extend(merged.iter().cloned());
        self.context
            .retain(|word| word.end_ms >= end.saturating_sub(OVERLAP_MS + TIMESTAMP_TOLERANCE_MS));
        self.previous_end = Some(end);
        let provisional = self
            .pending
            .iter()
            .map(|word| word.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        (captions(merged), provisional)
    }
}

fn midpoint(word: &Segment) -> u64 {
    word.start_ms + word.end_ms.saturating_sub(word.start_ms) / 2
}

fn key(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn align(old: &[Segment], new: &[Segment]) -> Vec<(usize, usize)> {
    let a: Vec<_> = old.iter().map(|word| key(&word.text)).collect();
    let b: Vec<_> = new.iter().map(|word| key(&word.text)).collect();
    let matches = |i: usize, j: usize| {
        !a[i].is_empty()
            && a[i] == b[j]
            && midpoint(&old[i]).abs_diff(midpoint(&new[j])) <= TIMESTAMP_TOLERANCE_MS
    };
    // Prefer the longest alignment, then the smallest timestamp drift. A
    // plain LCS can align repeated words to their neighbouring occurrence.
    let reward = (old.len().min(new.len()) as i64 + 1) * (TIMESTAMP_TOLERANCE_MS as i64 + 1);
    let weight = |i: usize, j: usize| reward - midpoint(&old[i]).abs_diff(midpoint(&new[j])) as i64;
    let mut scores = vec![vec![0_i64; new.len() + 1]; old.len() + 1];
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            let skip = scores[i + 1][j].max(scores[i][j + 1]);
            scores[i][j] = if matches(i, j) {
                skip.max(scores[i + 1][j + 1] + weight(i, j))
            } else {
                skip
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut result = Vec::new();
    while i < old.len() && j < new.len() {
        if matches(i, j) && scores[i][j] == scores[i + 1][j + 1] + weight(i, j) {
            result.push((i, j));
            i += 1;
            j += 1;
        } else if scores[i + 1][j] >= scores[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    result
}

// Keep exported subtitles readable: alignment uses words internally, not
// one-word subtitle cues. Never combine across a long silence.
fn captions(words: Vec<Segment>) -> Vec<Segment> {
    let mut result: Vec<Segment> = Vec::new();
    for word in words {
        if let Some(cue) = result.last_mut()
            && word.start_ms.saturating_sub(cue.end_ms) < 1_000
            && word.end_ms.saturating_sub(cue.start_ms) <= 6_000
            && cue.text.chars().count() + word.text.chars().count() < 80
        {
            cue.text.push(' ');
            cue.text.push_str(&word.text);
            cue.end_ms = word.end_ms;
        } else {
            result.push(word);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::Transcript;

    fn word(start: u64, text: &str) -> Segment {
        Segment {
            start_ms: start,
            end_ms: start + 400,
            text: text.into(),
        }
    }

    #[test]
    fn reconciles_changed_word_times_and_punctuation() {
        let mut state = Reconciler::default();
        let mut transcript = Transcript::default();
        let (first, provisional) = state.update(
            vec![word(25_000, "hello"), word(26_000, "world")],
            0,
            28_000,
            false,
        );
        transcript.append(first);
        assert_eq!(provisional, "world");
        let (last, provisional) = state.update(
            vec![
                word(25_200, "Hello,"),
                word(26_200, "world"),
                word(28_000, "again"),
            ],
            24_000,
            30_000,
            true,
        );
        transcript.append(last);
        assert_eq!(transcript.text(), "hello world again");
        assert!(provisional.is_empty());
        assert!(
            transcript
                .segments
                .windows(2)
                .all(|pair| pair[0].end_ms <= pair[1].start_ms)
        );
    }

    #[test]
    fn preserves_genuine_repeated_phrases() {
        let mut state = Reconciler::default();
        let mut transcript = Transcript::default();
        transcript.append(
            state
                .update(
                    vec![word(24_000, "go"), word(25_000, "go"), word(26_000, "go")],
                    0,
                    28_000,
                    false,
                )
                .0,
        );
        transcript.append(
            state
                .update(
                    vec![
                        word(24_100, "go"),
                        word(25_100, "go"),
                        word(26_100, "go"),
                        word(28_000, "go"),
                    ],
                    24_000,
                    30_000,
                    true,
                )
                .0,
        );
        assert_eq!(transcript.text(), "go go go go");
    }

    #[test]
    fn repeated_words_align_to_closest_occurrence() {
        let old = vec![word(25_000, "go"), word(26_000, "go")];
        let new = vec![word(24_100, "go"), word(25_100, "go"), word(26_100, "go")];
        assert_eq!(align(&old, &new), vec![(0, 1), (1, 2)]);
    }

    #[test]
    fn disagreement_uses_word_seam_not_entire_segment() {
        let mut state = Reconciler::default();
        state.update(
            vec![word(26_000, "earlier"), word(27_500, "uncertain")],
            0,
            28_000,
            false,
        );
        let (last, _) = state.update(
            vec![word(26_100, "revised"), word(28_000, "ending")],
            24_000,
            30_000,
            true,
        );
        assert_eq!(
            last.iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>()
                .join(" "),
            "revised ending"
        );
    }

    #[test]
    fn live_updates_and_multiple_stop_windows_publish_each_word_once() {
        let mut state = Reconciler::default();
        let mut transcript = Transcript::default();
        let words: Vec<_> = (0..60)
            .map(|i| word(i * 1_000, &format!("word{i}")))
            .collect();
        // Repeated growing live hypotheses, then several buffered Stop windows.
        for (start, end, last) in [
            (0, 3_000, false),
            (0, 6_000, false),
            (2_000, 10_000, false),
            (6_000, 34_000, false),
            (30_000, 58_000, false),
            (54_000, 60_000, true),
        ] {
            let hypothesis = words
                .iter()
                .filter(|w| w.start_ms >= start && w.end_ms <= end)
                .cloned()
                .collect();
            transcript.append(state.update(hypothesis, start, end, last).0);
        }
        assert_eq!(
            transcript.text(),
            words
                .iter()
                .map(|w| w.text.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        );
        assert!(
            transcript
                .segments
                .windows(2)
                .all(|pair| pair[0].end_ms <= pair[1].start_ms)
        );
    }

    #[test]
    fn exact_window_and_short_files_flush_all_words() {
        for end in [500, 28_000] {
            let mut state = Reconciler::default();
            let (finalized, provisional) =
                state.update(vec![word(end - 500, "last")], 0, end, true);
            assert_eq!(finalized[0].text, "last");
            assert!(provisional.is_empty());
        }
    }

    #[test]
    fn truncated_boundary_word_is_replaced_by_next_hypothesis() {
        let mut state = Reconciler::default();
        let mut transcript = Transcript::default();
        transcript.append(
            state
                .update(
                    vec![word(25_000, "a"), word(27_600, "bound")],
                    0,
                    28_000,
                    false,
                )
                .0,
        );
        transcript.append(
            state
                .update(
                    vec![
                        word(25_100, "a"),
                        word(27_600, "boundary"),
                        word(28_300, "word"),
                    ],
                    24_000,
                    30_000,
                    true,
                )
                .0,
        );
        assert_eq!(transcript.text(), "a boundary word");
    }

    #[test]
    fn silence_and_short_final_tails_finish_without_republishing() {
        let mut state = Reconciler::default();
        assert!(state.update(vec![], 0, 28_000, false).0.is_empty());
        let (last, provisional) = state.update(vec![word(28_000, "tail")], 24_000, 28_500, true);
        assert_eq!(last[0].text, "tail");
        assert!(provisional.is_empty());
    }
}
