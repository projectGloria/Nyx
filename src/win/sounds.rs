//! Preloaded typing samples. Audio work never blocks terminal input.
use crate::config::schema::SoundsConfig;
use crossbeam_channel::{unbounded, Sender};
use rodio::{OutputStream, Sink, Source};
use std::{
    io::Cursor,
    sync::{Arc, Mutex},
};
#[derive(Clone, Copy, Debug)]
pub enum Kind {
    Normal,
    Enter,
    Space,
    Backspace,
}
enum Command {
    Configure(SoundsConfig),
    Play(Kind),
}
pub struct Sounds {
    tx: Sender<Command>,
    pub status: Arc<Mutex<Option<String>>>,
}
pub fn packs() -> Vec<String> {
    let mut packs: Vec<_> = std::fs::read_dir(super::assets::root().join("sounds"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir() && e.path().join("normal.wav").is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    packs.sort_by_key(|s| s.to_lowercase());
    packs
}
impl Sounds {
    pub fn new(config: &SoundsConfig) -> Self {
        let (tx, rx) = unbounded();
        let status = Arc::new(Mutex::new(None));
        let report = status.clone();
        let spawn = std::thread::Builder::new()
            .name("nyx-audio".into())
            .spawn(move || {
                let mut stream = None;
                let mut samples = Vec::new();
                let mut volume = 0.5;
                let mut sinks: Vec<Sink> = Vec::new();
                while let Ok(command) = rx.recv() {
                    match command {
                        Command::Configure(config) => {
                            samples.clear();
                            sinks.clear();
                            volume = if config.volume.is_finite() {
                                config.volume.clamp(0.0, 1.0)
                            } else {
                                0.5
                            };
                            *report.lock().unwrap() = None;
                            if config.pack.is_empty() {
                                stream = None;
                                continue;
                            }
                            if !packs().contains(&config.pack) {
                                *report.lock().unwrap() =
                                    Some(format!("Sound pack '{}' is missing", config.pack));
                                continue;
                            }
                            let folder = super::assets::root().join("sounds").join(&config.pack);
                            match load_pack(&folder) {
                                Ok(loaded) => samples = loaded,
                                Err(e) => {
                                    *report.lock().unwrap() =
                                        Some(format!("Could not load sound pack: {e}"));
                                    continue;
                                }
                            }
                            if stream.is_none() {
                                match OutputStream::try_default() {
                                    Ok(output) => stream = Some(output),
                                    Err(e) => {
                                        samples.clear();
                                        *report.lock().unwrap() =
                                            Some(format!("Audio unavailable: {e}"));
                                    }
                                }
                            }
                        }
                        Command::Play(kind) => {
                            if volume == 0.0 {
                                continue;
                            }
                            if let (Some((_, handle)), Some(sample)) =
                                (&stream, samples.get(kind as usize))
                            {
                                if let Some(sink) = sinks.iter().find(|s| s.empty()) {
                                    sink.set_volume(volume);
                                    sink.append(sample.clone());
                                } else if sinks.len() < 32 {
                                    if let Ok(sink) = Sink::try_new(handle) {
                                        sink.set_volume(volume);
                                        sink.append(sample.clone());
                                        sinks.push(sink);
                                    }
                                }
                            }
                        }
                    }
                }
            });
        if let Err(err) = spawn {
            *status.lock().unwrap() = Some(format!("Audio worker unavailable: {err}"));
        }
        let worker = Self { tx, status };
        worker.configure(config);
        worker
    }
    pub fn configure(&self, config: &SoundsConfig) {
        let _ = self.tx.send(Command::Configure(config.clone()));
    }
    pub fn play(&self, kind: Kind) {
        if self.tx.len() < 256 {
            let _ = self.tx.try_send(Command::Play(kind));
        }
    }
    pub fn events(&self, events: &[egui::Event]) {
        for event in events {
            if let egui::Event::Key {
                key,
                pressed: true,
                modifiers: m,
                ..
            } = event
            {
                if ((m.ctrl || m.alt || m.command) && !(m.ctrl && m.alt))
                    || crate::config::keys::is_modifier(*key)
                {
                    continue;
                }
                let kind = match key {
                    egui::Key::Enter => Some(Kind::Enter),
                    egui::Key::Space => Some(Kind::Space),
                    egui::Key::Backspace => Some(Kind::Backspace),
                    _ if (key.name().len() == 1
                        || matches!(
                            key,
                            egui::Key::Minus
                                | egui::Key::Plus
                                | egui::Key::Equals
                                | egui::Key::Comma
                                | egui::Key::Period
                                | egui::Key::Slash
                                | egui::Key::Backslash
                                | egui::Key::Backtick
                                | egui::Key::OpenBracket
                                | egui::Key::CloseBracket
                                | egui::Key::Semicolon
                                | egui::Key::Quote
                        )) =>
                    {
                        Some(Kind::Normal)
                    }
                    _ => None,
                };
                if let Some(kind) = kind {
                    self.play(kind);
                }
            }
        }
    }
}

fn load_pack(folder: &std::path::Path) -> Result<Vec<rodio::buffer::SamplesBuffer<f32>>, String> {
    let load = |name: &str| -> Result<_, String> {
        let bytes = std::fs::read(folder.join(name)).map_err(|e| e.to_string())?;
        let decoder = rodio::Decoder::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
        let channels = decoder.channels();
        let rate = decoder.sample_rate();
        let data: Vec<f32> = decoder.convert_samples::<f32>().collect();
        if data.is_empty() {
            return Err(format!("{name} contains no audio"));
        }
        Ok(rodio::buffer::SamplesBuffer::new(channels, rate, data))
    };
    let normal = load("normal.wav")?;
    let mut samples = vec![normal.clone()];
    for name in ["enter.wav", "space.wav", "backspace.wav"] {
        samples.push(load(name).unwrap_or_else(|_| normal.clone()));
    }
    Ok(samples)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn supplied_release_sound_packs_decode_completely() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/release/sounds");
        if !root.exists() {
            return;
        }
        let mut count = 0;
        for folder in std::fs::read_dir(root)
            .unwrap()
            .flatten()
            .filter(|e| e.path().is_dir())
        {
            let samples = load_pack(&folder.path())
                .unwrap_or_else(|e| panic!("{}: {e}", folder.path().display()));
            assert_eq!(samples.len(), 4);
            for sample in samples {
                assert!(sample.count() > 0);
            }
            count += 1;
        }
        assert!(count > 0);
    }
    #[test]
    fn missing_specialized_samples_fall_back_to_normal() {
        let root = std::env::temp_dir().join(format!("nyx-audio-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let spec = rodio::buffer::SamplesBuffer::new(1, 8000, vec![0.0f32, 0.5, -0.5]);
        let file = root.join("normal.wav");
        let mut writer = hound_for_test(&file);
        for sample in spec {
            writer.write_sample((sample * 32767.0) as i16).unwrap();
        }
        writer.finalize().unwrap();
        let samples = load_pack(&root).unwrap();
        let normal: Vec<_> = samples[0].clone().collect();
        for sample in samples {
            assert_eq!(sample.collect::<Vec<_>>(), normal);
        }
        std::fs::remove_file(file).unwrap();
        assert!(load_pack(&root).is_err());
        std::fs::remove_dir(root).unwrap();
    }
    fn hound_for_test(
        path: &std::path::Path,
    ) -> hound::WavWriter<std::io::BufWriter<std::fs::File>> {
        hound::WavWriter::create(
            path,
            hound::WavSpec {
                channels: 1,
                sample_rate: 8000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap()
    }
}
