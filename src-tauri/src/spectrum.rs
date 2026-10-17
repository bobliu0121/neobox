//! FxSound-compatible 10-band IIR filter bank spectrum analyzer.
//!
//! Uses the exact coefficients from FxSound's spectrumReset.cpp:
//! 10 Butterworth 2-pole resonant bandpass filters (56Hz - 10kHz, log-spaced).
//! Each filter output is squared, smoothed (100ms 1-pole LP), then sqrt.
//! No FFT — pure IIR, sample-by-sample, zero latency (ignoring delay buffer).

#[derive(Clone)]
struct SpectrumBand {
    a1: f64,
    a2: f64,
    gain: f64,
    y1: f64,
    y2: f64,
    squared_filtered: f64,
    level: f64,
}

pub struct SpectrumAnalyzer {
    bands: Vec<SpectrumBand>,
    in_1: f64,
    in_2: f64,
    // Smoothing coefficient
    alpha: f64,
    one_minus_alpha: f64,
}

impl SpectrumAnalyzer {
    /// Create a new spectrum analyzer with FxSound-compatible coefficients.
    /// `sample_rate` is used to compute the smoothing time constant.
    pub fn new(sample_rate: f64) -> Self {
        // FxSound: alpha = exp(-time_constant / samp_freq), time_constant = 10 (100ms smooth)
        let alpha = (-10.0_f64 / sample_rate).exp();
        let one_minus_alpha = 1.0 - alpha;

        // Sensitivity and warp (from FxSound spectrumReset.cpp)
        let sensitivity: f64 = 4.5;
        let num_channels: f64 = 2.0; // stereo

        // Band centers: 56.23, 100, 177.83, 316.23, 562.34, 1000, 1778.28, 3162.28, 5623.4, 10000 Hz
        // Warp factors: [0.6, 0.6, 1.0, 1.0, 1.3, 1.3, 1.3, 1.3, 1.5, 1.5]

        Self {
            bands: vec![
                // Band 1: 56.23 Hz
                SpectrumBand {
                    a1: 1.9952707978,
                    a2: -0.9953348411,
                    gain: (sensitivity * 0.6) / (num_channels * 424.5657595),
                    y1: 0.0, y2: 0.0, squared_filtered: 0.0, level: 0.0,
                },
                // Band 2: 100 Hz
                SpectrumBand {
                    a1: 1.9915173377,
                    a2: -0.9917194870,
                    gain: (sensitivity * 0.6) / (num_channels * 239.1965397),
                    y1: 0.0, y2: 0.0, squared_filtered: 0.0, level: 0.0,
                },
                // Band 3: 177.83 Hz
                SpectrumBand {
                    a1: 1.9846835136,
                    a2: -0.9853206989,
                    gain: (sensitivity * 1.0) / (num_channels * 134.9296378),
                    y1: 0.0, y2: 0.0, squared_filtered: 0.0, level: 0.0,
                },
                // Band 4: 316.23 Hz
                SpectrumBand {
                    a1: 1.9720410075,
                    a2: -0.9740444157,
                    gain: (sensitivity * 1.0) / (num_channels * 76.31087758),
                    y1: 0.0, y2: 0.0, squared_filtered: 0.0, level: 0.0,
                },
                // Band 5: 562.34 Hz
                SpectrumBand {
                    a1: 1.9480305935,
                    a2: -0.9543009461,
                    gain: (sensitivity * 1.3) / (num_channels * 43.34342216),
                    y1: 0.0, y2: 0.0, squared_filtered: 0.0, level: 0.0,
                },
                // Band 6: 1000 Hz
                SpectrumBand {
                    a1: 1.9006550741,
                    a2: -0.9201218454,
                    gain: (sensitivity * 1.3) / (num_channels * 24.79951362),
                    y1: 0.0, y2: 0.0, squared_filtered: 0.0, level: 0.0,
                },
                // Band 7: 1778.28 Hz
                SpectrumBand {
                    a1: 1.8025225345,
                    a2: -0.8620772515,
                    gain: (sensitivity * 1.3) / (num_channels * 14.36694455),
                    y1: 0.0, y2: 0.0, squared_filtered: 0.0, level: 0.0,
                },
                // Band 8: 3162.28 Hz
                SpectrumBand {
                    a1: 1.5891186613,
                    a2: -0.7664106181,
                    gain: (sensitivity * 1.3) / (num_channels * 8.490790030),
                    y1: 0.0, y2: 0.0, squared_filtered: 0.0, level: 0.0,
                },
                // Band 9: 5623.4 Hz
                SpectrumBand {
                    a1: 1.1149497494,
                    a2: -0.6153052550,
                    gain: (sensitivity * 1.5) / (num_channels * 5.169741233),
                    y1: 0.0, y2: 0.0, squared_filtered: 0.0, level: 0.0,
                },
                // Band 10: 10000 Hz
                SpectrumBand {
                    a1: 0.1311997923,
                    a2: -0.3874425954,
                    gain: (sensitivity * 1.5) / (num_channels * 3.264452631),
                    y1: 0.0, y2: 0.0, squared_filtered: 0.0, level: 0.0,
                },
            ],
            in_1: 0.0,
            in_2: 0.0,
            alpha,
            one_minus_alpha,
        }
    }

    /// Process a single stereo sample pair through the filter bank.
    /// Returns the 10 band levels in range [0, 1].
    pub fn process(&mut self, left: f64, right: f64) -> [f64; 10] {
        let mono_in = left + right;

        // FxSound: input_sum = in - in_2 (difference from 2 samples ago)
        let input_sum = mono_in - self.in_2;

        // Process through all 10 bandpass filters
        for b in &mut self.bands {
            let mut out = input_sum + b.a1 * b.y1 + b.a2 * b.y2 + 1e-5;
            b.y2 = b.y1;
            b.y1 = out;
            out *= b.gain;

            // Square, smooth, sqrt
            let squared = out * out;
            b.squared_filtered =
                self.one_minus_alpha * squared + self.alpha * b.squared_filtered;

            // Fast sqrt approximation? No — f64::sqrt() is fast enough on modern CPUs
            if b.squared_filtered > 1.0 {
                b.level = 1.0;
            } else {
                b.level = b.squared_filtered.sqrt();
            }
        }

        // Shift input buffer
        self.in_2 = self.in_1;
        self.in_1 = mono_in;

        let mut levels = [0.0_f64; 10];
        for (i, b) in self.bands.iter().enumerate() {
            levels[i] = b.level.min(1.0);
        }
        levels
    }

    /// Reset all filter states to zero.
    #[allow(dead_code)]
    pub fn reset(&mut self) {
        self.in_1 = 0.0;
        self.in_2 = 0.0;
        for b in &mut self.bands {
            b.y1 = 0.0;
            b.y2 = 0.0;
            b.squared_filtered = 0.0;
            b.level = 0.0;
        }
    }
}
