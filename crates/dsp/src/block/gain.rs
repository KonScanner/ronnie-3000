/// Linear parameter ramp with a fixed number of samples.
#[derive(Debug, Clone)]
pub(crate) struct LinearGain {
    current: f32,
    target: f32,
    step: f32,
    countdown: usize,
    steps: usize,
}

impl LinearGain {
    pub(crate) fn new(sample_rate: f64, seconds: f64, initial: f32) -> Self {
        Self {
            current: initial,
            target: initial,
            step: 0.0,
            countdown: 0,
            steps: (seconds * sample_rate).floor() as usize,
        }
    }

    pub(crate) fn set_current_and_target(&mut self, value: f32) {
        self.current = value;
        self.target = value;
        self.step = 0.0;
        self.countdown = 0;
    }

    pub(crate) fn set_target(&mut self, value: f32) {
        if value == self.target {
            return;
        }
        self.target = value;
        if self.steps == 0 {
            self.set_current_and_target(value);
            return;
        }
        self.countdown = self.steps;
        self.step = (self.target - self.current) / self.steps as f32;
    }

    pub(crate) fn next(&mut self) -> f32 {
        if self.countdown == 0 {
            return self.target;
        }
        self.countdown -= 1;
        if self.countdown == 0 {
            self.current = self.target;
        } else {
            self.current += self.step;
        }
        self.current
    }

    pub(crate) fn current(&self) -> f32 {
        self.current
    }

    pub(crate) fn is_smoothing(&self) -> bool {
        self.countdown > 0
    }
}

pub(crate) fn db_to_gain(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

pub(crate) fn normalized_gain(value: f32) -> f32 {
    db_to_gain((value - 0.5) * 48.0)
}

pub(crate) fn peak_db(peak: f32) -> f32 {
    if peak > 0.0 {
        (20.0 * peak.log10()).max(-60.0)
    } else {
        -60.0
    }
}
