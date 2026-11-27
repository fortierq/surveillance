use std::{ffi::OsString, process::Stdio, sync::{Arc, Mutex}, time::Duration};

use serde::Serialize;
use surveillance_detect::{Motion, Region, Sound};
use tokio::{io::AsyncReadExt, process::Command, sync::watch};

use crate::{Result, config::{Config, WIDTH, HEIGHT}, store::{Kind, Store}};

const AUDIO_WINDOW: usize = 800;

#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SensorState {
    #[default]
    Disabled,
    Idle,
    Starting,
    Ready,
    #[cfg(target_os = "macos")]
    PermissionNeeded,
    #[cfg(target_os = "macos")]
    PermissionDenied,
    Unavailable,
}

#[derive(Default)]
pub struct Health(Mutex<(SensorState, SensorState)>);

impl Health {
    pub fn states(&self) -> (SensorState, SensorState) {
        *self.0.lock().unwrap()
    }

    fn set(&self, sensor: Sensor, state: SensorState) {
        let mut states = self.0.lock().unwrap();
        match sensor {
            Sensor::Camera => states.0 = state,
            Sensor::Microphone => states.1 = state,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Sensor { Camera, Microphone }

#[derive(Clone, Copy)]
enum Detection {
    Motion(Region, f32),
    Sound(f32),
}

pub struct Input {
    sensor: Sensor,
    device: String,
    detection: Detection,
}

impl Input {
    pub fn video(config: &Config) -> Option<Self> {
        config.camera_enabled.then_some(())?;
        let device = config.camera_device.clone().unwrap_or_else(|| {
            if cfg!(target_os = "linux") { "/dev/video0".into() } else { "0".into() }
        });
        Some(Self { sensor: Sensor::Camera, device, detection: Detection::Motion(config.region()?, config.motion_threshold) })
    }

    pub fn audio(config: &Config) -> Option<Self> {
        config.microphone_enabled.then_some(())?;
        let device = config.microphone_device.clone().unwrap_or_else(|| {
            if cfg!(target_os = "linux") { "default".into() } else { "0".into() }
        });
        Some(Self { sensor: Sensor::Microphone, device, detection: Detection::Sound(config.sound_margin_db) })
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(ffmpeg());
        cmd.args(["-hide_banner", "-nostdin", "-loglevel", "error"]);
        if cfg!(target_os = "macos") {
            cmd.args(["-f", "avfoundation"]);
            if matches!(self.sensor, Sensor::Camera) {
                cmd.args(["-framerate", "30", "-i", &format!("{}:none", self.device)]);
            } else {
                cmd.args(["-i", &format!("none:{}", self.device)]);
            }
        } else if cfg!(target_os = "windows") {
            let selector = match self.sensor { Sensor::Camera => "video", Sensor::Microphone => "audio" };
            cmd.args(["-f", "dshow", "-i", &format!("{selector}={}", self.device)]);
        } else {
            let format = match self.sensor { Sensor::Camera => "v4l2", Sensor::Microphone => "pulse" };
            cmd.args(["-f", format, "-i", &self.device]);
        }
        output_args(&mut cmd, self.sensor);
        cmd.stdin(Stdio::null()).stderr(Stdio::null()).stdout(Stdio::piped()).kill_on_drop(true);
        cmd
    }
}

fn output_args(cmd: &mut Command, sensor: Sensor) {
    match sensor {
        Sensor::Camera => {
            let filters = format!("fps=5,scale={WIDTH}:{HEIGHT},format=gray");
            cmd.args(["-an", "-vf", &filters, "-threads", "1", "-c:v", "rawvideo", "-f", "rawvideo"]);
        }
        Sensor::Microphone => {
            cmd.args(["-vn", "-ac", "1", "-ar", "16000", "-f", "s16le"]);
        }
    }
    cmd.arg("pipe:1");
}

pub async fn monitor(input: Input, store: Arc<Store>, health: Arc<Health>, mut armed: watch::Receiver<bool>) {
    let mut delay = Duration::from_secs(1);
    loop {
        if !*armed.borrow_and_update() {
            health.set(input.sensor, SensorState::Idle);
            delay = Duration::from_secs(1);
            if armed.changed().await.is_err() {
                return;
            }
            continue;
        }
        health.set(input.sensor, SensorState::Starting);
        tokio::select! {
            biased;
            change = armed.changed() => {
                if change.is_err() { return; }
                continue;
            }
            result = watch_input(&input, &store, &health, &mut delay) => {
                if let Err(error) = result {
                    tracing::warn!(sensor = ?input.sensor, %error, "Capture interrupted. Retrying.");
                    health.set(input.sensor, SensorState::Unavailable);
                }
            }
        }
        tokio::select! {
            change = armed.changed() => if change.is_err() { return; },
            _ = tokio::time::sleep(delay) => delay = (delay * 2).min(Duration::from_secs(15)),
        }
    }
}

async fn watch_input(input: &Input, store: &Store, health: &Health, delay: &mut Duration) -> Result<()> {
    #[cfg(target_os = "macos")]
    if !macos_permission(input.sensor, health).await {
        return Ok(());
    }
    let mut child = input.command().spawn()?;
    let mut samples = child.stdout.take().expect("FFmpeg stdout is piped");
    let (mut detector, mut buffer) = match input.detection {
        Detection::Motion(region, threshold) => (Detector::Motion(Motion::new(region, threshold)), vec![0; WIDTH * HEIGHT]),
        Detection::Sound(margin) => (Detector::Sound(Sound::new(margin)), vec![0; AUDIO_WINDOW * 2]),
    };
    tokio::time::timeout(Duration::from_secs(10), samples.read_exact(&mut buffer)).await??;
    health.set(input.sensor, SensorState::Ready);
    *delay = Duration::from_secs(1);
    loop {
        if let Some((kind, score)) = detector.observe(&buffer) {
            store.record(kind, score, true)?;
        }
        tokio::time::timeout(Duration::from_secs(8), samples.read_exact(&mut buffer)).await??;
    }
}

enum Detector { Motion(Motion), Sound(Sound) }

impl Detector {
    fn observe(&mut self, input: &[u8]) -> Option<(Kind, f32)> {
        match self {
            Self::Motion(detector) => detector.observe(input).map(|score| (Kind::Motion, score)),
            Self::Sound(detector) => {
                let mut window = [0; AUDIO_WINDOW];
                for (out, pair) in window.iter_mut().zip(input.chunks_exact(2)) {
                    *out = i16::from_le_bytes([pair[0], pair[1]]);
                }
                detector.observe(&window).map(|score| (Kind::Sound, score))
            }
        }
    }
}

#[cfg(target_os = "macos")]
async fn macos_permission(sensor: Sensor, health: &Health) -> bool {
    use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio, AVMediaTypeVideo};

    // AVFoundation defines these process-lifetime NSString constants.
    let media = unsafe {
        match sensor { Sensor::Camera => AVMediaTypeVideo, Sensor::Microphone => AVMediaTypeAudio }
    };
    let Some(media) = media else { return false; };
    match unsafe { AVCaptureDevice::authorizationStatusForMediaType(media) } {
        AVAuthorizationStatus::Authorized => true,
        AVAuthorizationStatus::NotDetermined => {
            health.set(sensor, SensorState::PermissionNeeded);
            let (sender, response) = tokio::sync::oneshot::channel();
            {
            let sender = Mutex::new(Some(sender));
            let callback = block2::RcBlock::new(move |granted: objc2::runtime::Bool| {
                if let Some(sender) = sender.lock().unwrap().take() {
                    let _ = sender.send(granted.as_bool());
                }
            });
            // AVFoundation retains the callback until it has a permission result.
            unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(media, &callback) };
            }
            if response.await.unwrap_or(false) { true } else {
                health.set(sensor, SensorState::PermissionDenied);
                false
            }
        }
        _ => {
            health.set(sensor, SensorState::PermissionDenied);
            false
        }
    }
}

fn ffmpeg() -> OsString {
    std::env::var_os("SURVEILLANCE_FFMPEG")
        .or_else(|| option_env!("SURVEILLANCE_BUILD_FFMPEG").map(OsString::from))
        .unwrap_or_else(|| "ffmpeg".into())
}

pub fn list_devices() -> Result<()> {
    let mut cmd = std::process::Command::new(ffmpeg());
    cmd.args(["-hide_banner", "-nostdin"]);
    if cfg!(target_os = "macos") {
        cmd.args(["-f", "avfoundation", "-list_devices", "true", "-i", ""]);
    } else if cfg!(target_os = "windows") {
        cmd.args(["-f", "dshow", "-list_devices", "true", "-i", "dummy"]);
    } else {
        for path in std::fs::read_dir("/dev")? {
            let path = path?;
            if path.file_name().to_string_lossy().starts_with("video") {
                println!("{}", path.path().display());
            }
        }
        cmd.args(["-sources", "pulse"]);
    }
    // FFmpeg device listing has no output stream and some backends exit with status 1.
    cmd.status()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires FFmpeg with lavfi"]
    async fn ffmpeg_outputs_motion_and_sound_for_detectors() {
        let video = concat!(
            "color=gray:s=160x120:r=5:d=3,",
            "drawbox=x=8:y=28:w=32:h=48:color=white:t=fill:enable='eq(n,3)',",
            "drawbox=x=48:y=28:w=32:h=48:color=white:t=fill:enable='eq(n,4)',",
            "drawbox=x=88:y=28:w=32:h=48:color=white:t=fill:enable='eq(n,5)'",
        );
        let motion = Detector::Motion(Motion::new(Config::default().region().unwrap(), 0.04));
        assert_eq!(fixture_count(video, Sensor::Camera, motion, WIDTH * HEIGHT).await, 1);

        let audio = "aevalsrc='if(between(t,0.5,0.9),0.4,0.02)*sin(2*PI*440*t)':s=16000:d=2";
        let sound = Detector::Sound(Sound::new(14.0));
        assert_eq!(fixture_count(audio, Sensor::Microphone, sound, AUDIO_WINDOW * 2).await, 1);
    }

    async fn fixture_count(source: &str, sensor: Sensor, mut detector: Detector, chunk_size: usize) -> usize {
        let mut generator = Command::new(ffmpeg());
        generator.args(["-v", "error", "-f", "lavfi", "-i", source]);
        output_args(&mut generator, sensor);
        let result = generator.output().await.unwrap();
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        assert!(!result.stdout.is_empty());
        assert_eq!(result.stdout.len() % chunk_size, 0);
        result.stdout.chunks_exact(chunk_size).filter_map(|sample| detector.observe(sample)).count()
    }
}
