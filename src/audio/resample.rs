//! Stateful windowed-sinc resampling. The low-pass cutoff prevents aliasing
//! when downsampling, and state is preserved across decoder/capture packets.
use std::f64::consts::PI;

pub struct Resampler {
    rate: u32,
    samples: Vec<f32>,
    // Rational phase avoids packet-size-dependent floating-point drift.
    position: u64,
}
const RADIUS: usize = 24;

impl Resampler {
    pub fn new(rate: u32) -> Self {
        assert!(rate > 0);
        Self {
            rate,
            samples: vec![0.0; RADIUS],
            position: RADIUS as u64 * super::SAMPLE_RATE as u64,
        }
    }

    pub fn push(&mut self, input: &[f32]) -> Vec<f32> {
        self.samples.extend_from_slice(input);
        let step = self.rate as f64 / super::SAMPLE_RATE as f64;
        let cutoff = (1.0 / step).min(1.0) * 0.94;
        let mut output = Vec::new();
        let denominator = super::SAMPLE_RATE as u64;
        while self.position + RADIUS as u64 * denominator < self.samples.len() as u64 * denominator
        {
            let center = (self.position / denominator) as usize;
            let mut sum = 0.0;
            let mut weight = 0.0;
            for index in center.saturating_sub(RADIUS - 1)..=center + RADIUS {
                let distance = index as f64 - self.position as f64 / denominator as f64;
                let x = PI * distance * cutoff;
                let sinc = if x.abs() < 1e-9 { 1.0 } else { x.sin() / x };
                let window = 0.5 + 0.5 * (PI * distance / RADIUS as f64).cos();
                let w = sinc * window * cutoff;
                sum += self.samples[index] as f64 * w;
                weight += w;
            }
            output.push((sum / weight) as f32);
            self.position += self.rate as u64;
        }
        let consumed = ((self.position / denominator) as usize)
            .saturating_sub(RADIUS)
            .min(self.samples.len());
        self.samples.drain(..consumed);
        self.position -= consumed as u64 * denominator;
        output
    }

    pub fn finish(&mut self) -> Vec<f32> {
        self.push(&[0.0; RADIUS])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packet_boundaries_do_not_change_audio() {
        let input: Vec<_> = (0..44100).map(|i| (i as f32 * 0.03).sin()).collect();
        let mut whole = Resampler::new(44100);
        let mut expected = whole.push(&input);
        expected.extend(whole.finish());
        let mut packets = Resampler::new(44100);
        let mut actual = Vec::new();
        for chunk in input.chunks(317) {
            actual.extend(packets.push(chunk));
        }
        actual.extend(packets.finish());
        assert!((actual.len() as i64 - 16000).abs() <= 1);
        assert_eq!(actual.len(), expected.len());
        assert!(
            actual
                .iter()
                .zip(expected)
                .all(|(a, b)| (a - b).abs() < 0.0001)
        );
    }
}
