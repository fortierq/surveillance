pub struct Motion {
    region: Region,
    threshold: f32,
    previous: Vec<u8>,
    activity: Activity,
}

#[derive(Clone, Copy)]
pub struct Region {
    width: usize,
    height: usize,
    left: usize,
    top: usize,
    right: usize,
    bottom: usize,
}

impl Region {
    pub fn new(width: usize, height: usize, [left, top, right, bottom]: [usize; 4]) -> Option<Self> {
        (left < right && top < bottom && right <= width && bottom <= height)
            .then_some(Self { width, height, left, top, right, bottom })
    }
}

impl Motion {
    pub fn new(region: Region, threshold: f32) -> Self {
        Self { region, threshold, previous: Vec::new(), activity: Activity::new(3, 10) }
    }

    // Input is an 8-bit grayscale raster, sampled at five frames per second.
    pub fn observe(&mut self, frame: &[u8]) -> Option<f32> {
        let Region { width, height, left, top, right, bottom } = self.region;
        if frame.len() != width * height {
            return None;
        }
        if self.previous.is_empty() {
            self.previous.extend_from_slice(frame);
            return None;
        }
        let mut deltas = [0usize; 511];
        for y in top..bottom {
            let row = y * width + left..y * width + right;
            for (&new, &old) in frame[row.clone()].iter().zip(&self.previous[row]) {
                deltas[(i32::from(new) - i32::from(old) + 255) as usize] += 1;
            }
        }
        self.previous.copy_from_slice(frame);
        let pixels = (right - left) * (bottom - top);
        let mut cumulative = 0;
        let median = deltas.iter().position(|count| {
            cumulative += count;
            cumulative > pixels / 2
        }).unwrap_or(255);
        let changed: usize = deltas.iter().enumerate()
            .filter(|(delta, _)| delta.abs_diff(median) >= 18)
            .map(|(_, count)| count)
            .sum();
        let score = changed as f32 / pixels as f32;
        self.activity.observe(score, self.threshold, self.threshold * 0.6, false)
    }
}

pub struct Sound {
    noise_floor: Option<f32>,
    margin_db: f32,
    activity: Activity,
}

impl Sound {
    pub fn new(margin_db: f32) -> Self {
        Self { noise_floor: None, margin_db, activity: Activity::new(2, 20) }
    }

    // Input is mono PCM in 50 ms windows. Sharp impulses can trigger in one window.
    pub fn observe(&mut self, window: &[i16]) -> Option<f32> {
        if window.is_empty() {
            return None;
        }
        let power = window.iter().map(|&sample| (f32::from(sample) / 32768.0).powi(2)).sum::<f32>() / window.len() as f32;
        let db = 10.0 * power.max(1e-8).log10();
        let floor = self.noise_floor.get_or_insert(db.min(-35.0));
        let score = db - floor.max(-65.0);
        let impulse = score >= self.margin_db + 14.0;
        let event = self.activity.observe(score, self.margin_db, self.margin_db * 0.65, impulse);
        if !self.activity.active && self.activity.above == 0 {
            *floor += (db - *floor) * 0.02;
        }
        event
    }
}

struct Activity {
    active: bool,
    above: usize,
    below: usize,
    trigger_frames: usize,
    release_frames: usize,
}

impl Activity {
    fn new(trigger_frames: usize, release_frames: usize) -> Self {
        Self { active: false, above: 0, below: 0, trigger_frames, release_frames }
    }

    fn observe(&mut self, score: f32, trigger: f32, release: f32, impulse: bool) -> Option<f32> {
        if self.active {
            self.below = if score < release { self.below + 1 } else { 0 };
            if self.below >= self.release_frames {
                self.active = false;
                self.below = 0;
            }
        } else {
            self.above = if score >= trigger { self.above + 1 } else { 0 };
            if impulse || self.above >= self.trigger_frames {
                self.active = true;
                self.above = 0;
                return Some(score);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motion_filters_lighting_and_groups_activity() {
        let mut motion = Motion::new(Region::new(20, 10, [0, 0, 20, 10]).unwrap(), 0.05);
        let mut frame = vec![25; 200];
        assert_eq!(motion.observe(&frame), None);
        frame.fill(90);
        assert_eq!(motion.observe(&frame), None);
        for offset in [0, 20, 40, 60, 80, 100] {
            frame.fill(90);
            frame[offset..offset + 20].fill(200);
            assert_eq!(motion.observe(&frame).is_some(), offset == 40);
        }
        for _ in 0..10 {
            assert_eq!(motion.observe(&frame), None);
        }
        for offset in [0, 20, 40] {
            frame.fill(90);
            frame[offset..offset + 20].fill(200);
            assert_eq!(motion.observe(&frame).is_some(), offset == 40);
        }
    }

    #[test]
    fn sound_groups_loud_activity_over_a_steady_floor() {
        let mut sound = Sound::new(12.0);
        for _ in 0..120 {
            assert_eq!(sound.observe(&[700; 800]), None);
        }
        assert_eq!(sound.observe(&[9500; 800]), None);
        assert!(sound.observe(&[9500; 800]).is_some());
        for _ in 0..40 {
            assert_eq!(sound.observe(&[9500; 800]), None);
        }
        for _ in 0..25 {
            assert_eq!(sound.observe(&[700; 800]), None);
        }
        assert_eq!(sound.observe(&[9500; 800]), None);
        assert!(sound.observe(&[9500; 800]).is_some());
    }

    #[test]
    fn a_single_loud_clap_triggers_sound_detection() {
        let mut sound = Sound::new(14.0);
        for _ in 0..10 {
            assert_eq!(sound.observe(&[220; 800]), None);
        }
        assert!(sound.observe(&[13000; 800]).is_some());
        assert_eq!(sound.observe(&[220; 800]), None);
    }
}
