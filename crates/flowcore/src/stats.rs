// Running statistics accumulator: count, mean, sd, min, max -- constant
// memory, no stored samples.

#[derive(Clone, Copy, Debug, Default)]
pub struct Acc {
    pub n: u64,
    sum: f64,
    sumsq: f64,
    pub min: f64,
    pub max: f64,
}

impl Acc {
    pub fn add(&mut self, v: f64) {
        if self.n == 0 {
            self.min = v;
            self.max = v;
        } else {
            if v < self.min {
                self.min = v;
            }
            if v > self.max {
                self.max = v;
            }
        }
        self.n += 1;
        self.sum += v;
        self.sumsq += v * v;
    }
    pub fn mean(&self) -> f64 {
        if self.n == 0 {
            0.0
        } else {
            self.sum / self.n as f64
        }
    }
    pub fn sd(&self) -> f64 {
        if self.n < 2 {
            return 0.0;
        }
        let m = self.mean();
        ((self.sumsq / self.n as f64) - m * m).max(0.0).sqrt()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acc_stats() {
        let mut a = Acc::default();
        for v in [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0] {
            a.add(v);
        }
        assert_eq!(a.n, 8);
        assert!((a.mean() - 5.0).abs() < 1e-12);
        assert!((a.sd() - 2.0).abs() < 1e-12);
        assert_eq!(a.min, 2.0);
        assert_eq!(a.max, 9.0);
    }
}
