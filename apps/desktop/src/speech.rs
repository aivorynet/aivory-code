//! The offline dictation: a speech model that runs on this machine. The Windows recognizer
//! needs "Online speech recognition" in the Windows settings; this engine does not. The model
//! (a streaming zipformer transducer, Apache-2.0) is not part of the app: the user agrees to
//! download it once into `<cache>/models`. The engine is `sherpa-onnx`; the microphone is `cpal`.
//!
//! The download and the unpacking run the programs of Windows (`curl`, `tar`), so the app needs
//! no HTTP or archive library.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The model: the streaming zipformer for English of the sherpa-onnx releases (LibriSpeech,
/// Apache-2.0, 20 million parameters).
pub const MODEL_NAME: &str = "sherpa-onnx-streaming-zipformer-en-20M-2023-02-17";

pub fn model_url() -> String {
    format!(
        "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/{MODEL_NAME}.tar.bz2"
    )
}

/// The folder of the models: `<cache>/models`.
pub fn models_dir() -> PathBuf {
    crate::compose::cache_base().join("models")
}

/// The four files of an installed model, under fixed names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelFiles {
    pub encoder: PathBuf,
    pub decoder: PathBuf,
    pub joiner: PathBuf,
    pub tokens: PathBuf,
}

impl ModelFiles {
    fn in_dir(dir: &Path) -> Self {
        Self {
            encoder: dir.join("encoder.onnx"),
            decoder: dir.join("decoder.onnx"),
            joiner: dir.join("joiner.onnx"),
            tokens: dir.join("tokens.txt"),
        }
    }

    fn complete(&self) -> bool {
        [&self.encoder, &self.decoder, &self.joiner, &self.tokens]
            .iter()
            .all(|p| p.is_file())
    }
}

/// The installed model, when all its files are there.
pub fn installed() -> Option<ModelFiles> {
    installed_in(&models_dir())
}

fn installed_in(models: &Path) -> Option<ModelFiles> {
    let files = ModelFiles::in_dir(&models.join(MODEL_NAME));
    files.complete().then_some(files)
}

/// The files of an unpacked archive: the int8 variant of each network (smaller and faster),
/// else the plain one. `None` when a file is missing.
pub fn pick_files(unpacked: &Path) -> Option<ModelFiles> {
    let names: Vec<String> = std::fs::read_dir(unpacked)
        .ok()?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .collect();
    let pick = |prefix: &str| -> Option<PathBuf> {
        let onnx = |n: &&String| n.starts_with(prefix) && n.ends_with(".onnx");
        let chosen = names
            .iter()
            .filter(onnx)
            .find(|n| n.contains("int8"))
            .or_else(|| names.iter().find(onnx))?;
        Some(unpacked.join(chosen))
    };
    let tokens = unpacked.join("tokens.txt");
    Some(ModelFiles {
        encoder: pick("encoder")?,
        decoder: pick("decoder")?,
        joiner: pick("joiner")?,
        tokens: tokens.is_file().then_some(tokens)?,
    })
}

/// `122 MB`: the size of a download for the consent line.
pub fn megabytes(bytes: u64) -> u64 {
    bytes.div_ceil(1_000_000)
}

/// The `Content-Length` of the last response in the headers of a followed redirect.
pub fn parse_content_length(headers: &str) -> Option<u64> {
    headers
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<u64>().ok())?
        })
        .next_back()
        .filter(|n| *n > 0)
}

/// The program of Windows with this name, else the name (the PATH finds it).
fn system_tool(name: &str) -> PathBuf {
    if cfg!(windows)
        && let Some(root) = std::env::var_os("SystemRoot")
    {
        let path = Path::new(&root)
            .join("System32")
            .join(format!("{name}.exe"));
        if path.is_file() {
            return path;
        }
    }
    PathBuf::from(name)
}

/// The size of the model download in bytes, from the headers of the release.
pub fn download_size() -> Option<u64> {
    let out = Command::new(system_tool("curl"))
        .args(["-sIL", "--max-time", "15", &model_url()])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    parse_content_length(&String::from_utf8_lossy(&out.stdout))
}

/// How far the download is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    Downloading { done: u64, total: Option<u64> },
    Unpacking,
}

/// The line of a progress: `Downloading the speech model… 37% (45 of 122 MB)`.
pub fn progress_text(p: Progress) -> String {
    match p {
        Progress::Downloading {
            done,
            total: Some(total),
        } if total > 0 => format!(
            "Downloading the speech model… {}% ({} of {} MB)",
            (done.min(total) * 100 / total),
            done / 1_000_000,
            megabytes(total)
        ),
        Progress::Downloading { done, .. } => {
            format!("Downloading the speech model… {} MB", done / 1_000_000)
        }
        Progress::Unpacking => "Unpacking the speech model…".to_string(),
    }
}

/// Download the model and install it. `progress` is called a few times per second; a set
/// `cancel` flag stops the download. A half download stays out of the models folder.
pub fn download_and_install(
    total: Option<u64>,
    cancel: &AtomicBool,
    progress: &dyn Fn(Progress),
) -> Result<(), String> {
    let models = models_dir();
    std::fs::create_dir_all(&models)
        .map_err(|e| format!("Cannot create {}: {e}", models.display()))?;
    let archive = models.join(format!("{MODEL_NAME}.tar.bz2.part"));
    let _ = std::fs::remove_file(&archive);
    let mut child = Command::new(system_tool("curl"))
        .args(["-L", "--fail", "-s", "--retry", "2", "-o"])
        .arg(&archive)
        .arg(model_url())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Cannot start curl: {e}"))?;
    let status = wait_with_progress(&mut child, &archive, total, cancel, progress);
    match status {
        Ok(true) => {}
        Ok(false) => {
            let _ = std::fs::remove_file(&archive);
            return Err("The download was cancelled.".to_string());
        }
        Err(e) => {
            let _ = std::fs::remove_file(&archive);
            return Err(e);
        }
    }
    progress(Progress::Unpacking);
    let result = install_from_archive(&archive, &models);
    let _ = std::fs::remove_file(&archive);
    result
}

/// Wait for the download. `Ok(false)`: cancelled.
fn wait_with_progress(
    child: &mut Child,
    archive: &Path,
    total: Option<u64>,
    cancel: &AtomicBool,
    progress: &dyn Fn(Progress),
) -> Result<bool, String> {
    loop {
        if cancel.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(false);
        }
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(true),
            Ok(Some(_)) => return Err("The download failed. Check the network.".to_string()),
            Ok(None) => {}
            Err(e) => return Err(format!("The download failed: {e}")),
        }
        let done = std::fs::metadata(archive).map_or(0, |m| m.len());
        progress(Progress::Downloading { done, total });
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// Unpack an archive of the model into `<models>/<MODEL_NAME>` with the four files under
/// fixed names. The rest of the archive is dropped.
pub fn install_from_archive(archive: &Path, models: &Path) -> Result<(), String> {
    let work = models.join(format!("{MODEL_NAME}.unpack"));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| format!("Cannot create {}: {e}", work.display()))?;
    let status = Command::new(system_tool("tar"))
        .arg("-xjf")
        .arg(archive)
        .arg("-C")
        .arg(&work)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("Cannot start tar: {e}"));
    let result = status.and_then(|s| {
        if s.success() {
            Ok(())
        } else {
            Err("The model archive did not unpack.".to_string())
        }
    });
    let result = result.and_then(|()| {
        let unpacked = work.join(MODEL_NAME);
        let found = pick_files(&unpacked).ok_or("The model archive misses a file.")?;
        let target = models.join(MODEL_NAME);
        std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
        let fixed = ModelFiles::in_dir(&target);
        for (from, to) in [
            (&found.encoder, &fixed.encoder),
            (&found.decoder, &fixed.decoder),
            (&found.joiner, &fixed.joiner),
            (&found.tokens, &fixed.tokens),
        ] {
            std::fs::copy(from, to).map_err(|e| format!("Cannot copy {}: {e}", from.display()))?;
        }
        Ok(())
    });
    let _ = std::fs::remove_dir_all(&work);
    result
}

// ---- the recognizer -------------------------------------------------------------------------

/// The sample rate that the model wants.
pub const MODEL_RATE: u32 = 16_000;

/// The mean of the channels of an interleaved buffer.
pub fn downmix(data: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return data.to_vec();
    }
    data.chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// A linear resampler that keeps its place between buffers.
#[derive(Debug)]
pub struct Resampler {
    /// Input samples per output sample.
    step: f64,
    /// The position of the next output sample, in input samples after `last`.
    pos: f64,
    last: f32,
}

impl Resampler {
    pub fn new(from: u32, to: u32) -> Self {
        Self {
            step: f64::from(from) / f64::from(to),
            pos: 0.,
            last: 0.,
        }
    }

    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        let mut out = Vec::with_capacity((input.len() as f64 / self.step) as usize + 1);
        // The sample before the buffer is `last`; sample i of the buffer is at position i + 1.
        let at = |i: usize, last: f32| if i == 0 { last } else { input[i - 1] };
        while self.pos < input.len() as f64 {
            let i = self.pos.floor() as usize;
            let frac = (self.pos - self.pos.floor()) as f32;
            let a = at(i, self.last);
            let b = input[i];
            out.push(a + (b - a) * frac);
            self.pos += self.step;
        }
        self.pos -= input.len() as f64;
        if let Some(l) = input.last() {
            self.last = *l;
        }
        out
    }
}

/// The text of the model (capital letters, no punctuation) as a sentence.
pub fn tidy(text: &str) -> String {
    let lower = text.trim().to_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut start = true;
    for word in lower.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        // The pronoun "I" stays a capital.
        let word = match word {
            "i" => "I".to_string(),
            w if w.starts_with("i'") => format!("I{}", &w[1..]),
            w => w.to_string(),
        };
        if start {
            let mut chars = word.chars();
            if let Some(first) = chars.next() {
                out.extend(first.to_uppercase());
                out.push_str(chars.as_str());
            }
            start = false;
        } else {
            out.push_str(&word);
        }
    }
    out
}

#[cfg(windows)]
mod engine {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use anyhow::{Result, anyhow};
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use sherpa_onnx::{
        OnlineModelConfig, OnlineRecognizer, OnlineRecognizerConfig, OnlineStream,
        OnlineTransducerModelConfig,
    };

    use super::{MODEL_RATE, ModelFiles, Resampler, downmix, tidy};

    /// The recognizer of a model.
    fn recognizer(files: &ModelFiles) -> Result<OnlineRecognizer> {
        let path = |p: &std::path::Path| Some(p.to_string_lossy().into_owned());
        let config = OnlineRecognizerConfig {
            model_config: OnlineModelConfig {
                transducer: OnlineTransducerModelConfig {
                    encoder: path(&files.encoder),
                    decoder: path(&files.decoder),
                    joiner: path(&files.joiner),
                },
                tokens: path(&files.tokens),
                num_threads: 2,
                ..Default::default()
            },
            decoding_method: Some("greedy_search".to_string()),
            enable_endpoint: true,
            rule1_min_trailing_silence: 3.0,
            rule2_min_trailing_silence: 1.0,
            rule3_min_utterance_length: 20.0,
            ..Default::default()
        };
        OnlineRecognizer::create(&config)
            .ok_or_else(|| anyhow!("the speech model did not load; delete it and download again"))
    }

    /// Decode what the stream holds. Returns the text so far and whether the speaker paused.
    fn pump(r: &OnlineRecognizer, stream: &OnlineStream) -> (String, bool) {
        while r.is_ready(stream) {
            r.decode(stream);
        }
        let text = r.get_result(stream).map(|x| x.text).unwrap_or_default();
        (text, r.is_endpoint(stream))
    }

    /// Recognize samples at 16 kHz (a recording, a test).
    #[cfg(test)]
    pub fn recognize_samples(files: &ModelFiles, samples: &[f32]) -> Result<String> {
        let r = recognizer(files)?;
        let stream = r.create_stream();
        stream.accept_waveform(MODEL_RATE as i32, samples);
        // A second of silence lets the model finish its last word.
        stream.accept_waveform(MODEL_RATE as i32, &vec![0.; MODEL_RATE as usize]);
        stream.input_finished();
        let (text, _) = pump(&r, &stream);
        Ok(tidy(&text))
    }

    /// Dictate with the default microphone until the speaker pauses, the stop flag is set, or
    /// 60 s pass. The words so far are kept on a stop.
    pub fn recognize_microphone(files: &ModelFiles, stop: &AtomicBool) -> Result<String> {
        let r = recognizer(files)?;
        let stream = r.create_stream();
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| anyhow!("no microphone was found"))?;
        let config = device
            .default_input_config()
            .map_err(|e| anyhow!("the microphone has no setting: {e}"))?;
        let (channels, rate) = (usize::from(config.channels()), config.sample_rate());
        let (tx, rx) = mpsc::channel::<Vec<f32>>();
        let err_fn = |e: cpal::Error| eprintln!("aivory: microphone: {e}");
        let built = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                config.config(),
                move |data: &[f32], _: &_| {
                    let _ = tx.send(downmix(data, channels));
                },
                err_fn,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_input_stream(
                config.config(),
                move |data: &[i16], _: &_| {
                    let floats: Vec<f32> = data.iter().map(|s| f32::from(*s) / 32768.).collect();
                    let _ = tx.send(downmix(&floats, channels));
                },
                err_fn,
                None,
            ),
            other => return Err(anyhow!("the microphone format {other} is not supported")),
        };
        let input = built.map_err(|e| anyhow!("the microphone did not open: {e}"))?;
        input
            .play()
            .map_err(|e| anyhow!("the microphone did not start: {e}"))?;
        let mut resampler = Resampler::new(rate, MODEL_RATE);
        let started = Instant::now();
        let mut heard_audio = false;
        let mut segments: Vec<String> = Vec::new();
        let mut current = String::new();
        loop {
            if stop.load(Ordering::SeqCst) || started.elapsed() > Duration::from_secs(60) {
                break;
            }
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(chunk) => {
                    heard_audio = true;
                    stream.accept_waveform(MODEL_RATE as i32, &resampler.process(&chunk));
                    let (text, pause) = pump(&r, &stream);
                    if !text.trim().is_empty() {
                        current = text;
                    }
                    if pause {
                        if !current.trim().is_empty() {
                            segments.push(std::mem::take(&mut current));
                            break;
                        }
                        r.reset(&stream);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if !heard_audio && started.elapsed() > Duration::from_secs(3) {
                        return Err(anyhow!(
                            "the microphone gives no sound (0x80070005): is it allowed for desktop apps?"
                        ));
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        drop(input);
        stream.input_finished();
        let (text, _) = pump(&r, &stream);
        if !text.trim().is_empty() {
            current = text;
        }
        if !current.trim().is_empty() {
            segments.push(current);
        }
        Ok(tidy(&segments.join(" ")))
    }
}

#[cfg(windows)]
pub use engine::recognize_microphone;
#[cfg(all(windows, test))]
pub use engine::recognize_samples;

#[cfg(not(windows))]
pub fn recognize_microphone(_: &ModelFiles, _: &AtomicBool) -> anyhow::Result<String> {
    Err(anyhow::anyhow!("the offline dictation needs Windows"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("aivory-speech-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch folder");
        dir
    }

    #[test]
    fn the_size_comes_from_the_last_response() {
        let headers = "HTTP/1.1 302 Found\r\nContent-Length: 0\r\n\r\nHTTP/1.1 200 OK\r\ncontent-length: 127887156\r\n\r\n";
        assert_eq!(parse_content_length(headers), Some(127_887_156));
        assert_eq!(parse_content_length("HTTP/1.1 404\r\n"), None);
        assert_eq!(parse_content_length("Content-Length: 0\r\n"), None);
        assert_eq!(megabytes(127_887_156), 128);
        assert_eq!(megabytes(1), 1);
    }

    #[test]
    fn the_progress_line_names_percent_and_megabytes() {
        let p = Progress::Downloading {
            done: 45_000_000,
            total: Some(120_000_000),
        };
        assert_eq!(
            progress_text(p),
            "Downloading the speech model… 37% (45 of 120 MB)"
        );
        assert_eq!(
            progress_text(Progress::Downloading {
                done: 5_000_000,
                total: None
            }),
            "Downloading the speech model… 5 MB"
        );
        assert_eq!(
            progress_text(Progress::Unpacking),
            "Unpacking the speech model…"
        );
        // A total that grows past the size in the headers never shows more than 100 percent.
        let over = Progress::Downloading {
            done: 130_000_000,
            total: Some(120_000_000),
        };
        assert!(progress_text(over).contains("100%"));
    }

    #[test]
    fn the_int8_files_win_and_all_four_must_exist() {
        let dir = scratch("pick");
        for name in [
            "encoder-epoch-99-avg-1.onnx",
            "encoder-epoch-99-avg-1.int8.onnx",
            "decoder-epoch-99-avg-1.onnx",
            "joiner-epoch-99-avg-1.int8.onnx",
        ] {
            std::fs::write(dir.join(name), b"x").expect("write");
        }
        assert!(
            pick_files(&dir).is_none(),
            "tokens and the plain decoder miss"
        );
        std::fs::write(dir.join("tokens.txt"), b"x").expect("write");
        let files = pick_files(&dir).expect("complete");
        assert!(
            files
                .encoder
                .to_string_lossy()
                .ends_with("encoder-epoch-99-avg-1.int8.onnx")
        );
        assert!(
            files
                .decoder
                .to_string_lossy()
                .ends_with("decoder-epoch-99-avg-1.onnx")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_model_counts_as_installed_only_with_all_files() {
        let models = scratch("installed");
        assert!(installed_in(&models).is_none());
        let target = models.join(MODEL_NAME);
        std::fs::create_dir_all(&target).expect("folder");
        for name in ["encoder.onnx", "decoder.onnx", "joiner.onnx"] {
            std::fs::write(target.join(name), b"x").expect("write");
        }
        assert!(installed_in(&models).is_none());
        std::fs::write(target.join("tokens.txt"), b"x").expect("write");
        assert!(installed_in(&models).is_some());
        let _ = std::fs::remove_dir_all(&models);
    }

    #[test]
    fn two_channels_become_one() {
        assert_eq!(downmix(&[1., 0., 0.5, 0.5], 2), [0.5, 0.5]);
        assert_eq!(downmix(&[1., 2.], 1), [1., 2.]);
    }

    #[test]
    fn the_resampler_keeps_the_rate_ratio_over_many_buffers() {
        let mut r = Resampler::new(48_000, 16_000);
        let mut total = 0;
        for _ in 0..100 {
            total += r.process(&[0.25; 480]).len();
        }
        // 48 000 input samples make 16 000 output samples; a buffer edge may shift one.
        assert!((15_999..=16_001).contains(&total), "{total}");
        // Up-sampling (8 kHz to 16 kHz) doubles the count and keeps a ramp straight.
        let mut up = Resampler::new(8_000, 16_000);
        let out = up.process(&[0., 1., 2., 3.]);
        assert_eq!(out.len(), 8);
        assert!(out.windows(2).all(|w| w[1] >= w[0]));
    }

    #[test]
    fn the_model_text_becomes_a_sentence() {
        assert_eq!(tidy("HELLO WORLD"), "Hello world");
        assert_eq!(tidy("  i THINK i'M here "), "I think I'm here");
        assert_eq!(tidy(""), "");
    }

    #[test]
    fn the_model_lives_under_the_cache() {
        assert!(models_dir().ends_with("models"));
        assert!(model_url().ends_with(".tar.bz2"));
    }

    /// Downloads the model (about 120 MB) and recognizes its test recording. Needs the network:
    /// `cargo test -p aivory-desktop -- --ignored the_model_recognizes`.
    #[cfg(windows)]
    #[test]
    #[ignore = "downloads the speech model"]
    fn the_model_recognizes_its_test_recording() {
        let models = scratch("full");
        let archive = models.join("model.tar.bz2");
        let status = Command::new(system_tool("curl"))
            .args(["-L", "--fail", "-s", "-o"])
            .arg(&archive)
            .arg(model_url())
            .status()
            .expect("curl");
        assert!(status.success(), "the download");
        let wavs = models.join("wavs");
        std::fs::create_dir_all(&wavs).expect("folder");
        let status = Command::new(system_tool("tar"))
            .arg("-xjf")
            .arg(&archive)
            .arg("-C")
            .arg(&wavs)
            .arg(format!("{MODEL_NAME}/test_wavs/0.wav"))
            .status()
            .expect("tar");
        assert!(status.success(), "the unpack of the test recording");
        install_from_archive(&archive, &models).expect("install");
        let files = installed_in(&models).expect("installed");
        let wav = sherpa_onnx::Wave::read(
            &wavs
                .join(MODEL_NAME)
                .join("test_wavs")
                .join("0.wav")
                .to_string_lossy(),
        )
        .expect("the test recording");
        assert_eq!(wav.sample_rate(), 16_000);
        let text = recognize_samples(&files, wav.samples()).expect("recognized");
        eprintln!("recognized: {text}");
        assert!(text.split_whitespace().count() >= 5, "{text}");
        let _ = std::fs::remove_dir_all(&models);
    }
}
