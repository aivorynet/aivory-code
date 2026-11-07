//! The tools of a composer: attach files, capture the screen, dictate. A file or a capture
//! becomes a chip over the input (its name and a cross); on send the chips go to the daemon as
//! `@<path>` references. A dictation puts the spoken words into the input. The state of the
//! tools ([`Tools`]) lives in the screen; a tool shows its result, or its failure, in a visible
//! line that hides after [`notice::HIDE_AFTER`].

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, anyhow};
use gpui::prelude::*;
use gpui::{
    Context, Div, Entity, FontWeight, PathPromptOptions, SharedString, Window, div, px, rgba,
};
use gpui_base::input::TextareaState;

use crate::notice::{self, Help};
use crate::speech::{self, Progress};
use crate::theme::*;
use crate::ui::{self, OnClick};

/// The mic button shows only where a speech recognizer exists.
pub const VOICE_AVAILABLE: bool = cfg!(windows);

/// Windows setting "Online speech recognition" is off (`SPERR_SPEECH_PRIVACY_POLICY_NOT_ACCEPTED`).
pub const SPEECH_POLICY_OFF: u32 = 0x8004_5509;
/// Windows blocks the microphone for desktop apps (`E_ACCESSDENIED`).
pub const ACCESS_DENIED: u32 = 0x8007_0005;

/// A file or a capture on the composer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub path: PathBuf,
    /// The file name, shown on the chip.
    pub name: String,
    /// A screen capture and not a picked file.
    pub capture: bool,
}

impl Attachment {
    pub fn new(path: PathBuf, capture: bool) -> Self {
        let name = path.file_name().map_or_else(
            || path.to_string_lossy().into_owned(),
            |n| n.to_string_lossy().into_owned(),
        );
        Self {
            path,
            name,
            capture,
        }
    }
}

/// A line that tells the user what a tool did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub id: u64,
    pub text: String,
    pub failure: bool,
    pub help: Option<Help>,
}

/// The state of the offline speech model in a composer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Offline {
    #[default]
    Idle,
    /// The model is not installed: the composer asks for the download. `bytes` is the size.
    Offer { bytes: Option<u64> },
    /// The download or the unpacking runs.
    Busy(Progress),
}

/// What a button of the tool line does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolAction {
    /// Stop the dictation; the words so far stay.
    StopListening,
    /// The user agrees to the download.
    Download,
    /// The user does not want the download now.
    Decline,
    CancelDownload,
}

/// The state of the tools of one composer.
#[derive(Debug, Default)]
pub struct Tools {
    pub attachments: Vec<Attachment>,
    pub capturing: bool,
    /// The dictation runs; the flag asks it to stop.
    pub listening: Option<Arc<AtomicBool>>,
    pub note: Option<Note>,
    pub offline: Offline,
    /// The download of the model runs; the flag cancels it.
    download: Option<Arc<AtomicBool>>,
    next: u64,
}

impl Tools {
    /// A tool runs now.
    pub fn busy(&self) -> bool {
        self.capturing || self.listening.is_some()
    }

    /// The composer has something to show: it stays open.
    pub fn active(&self) -> bool {
        !self.attachments.is_empty()
            || self.busy()
            || self.note.is_some()
            || self.offline != Offline::Idle
    }

    pub fn add_files(&mut self, paths: Vec<PathBuf>, capture: bool) {
        for path in paths {
            if !self.attachments.iter().any(|a| a.path == path) {
                self.attachments.push(Attachment::new(path, capture));
            }
        }
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.attachments.len() {
            self.attachments.remove(index);
        }
    }

    /// Ask a running dictation to stop. `false`: none runs.
    pub fn stop_listening(&mut self) -> bool {
        match &self.listening {
            Some(flag) => {
                flag.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }

    /// Start a dictation: the stop flag for the recognizer, or `None` when one runs already.
    pub fn start_listening(&mut self) -> Option<Arc<AtomicBool>> {
        if self.listening.is_some() {
            return None;
        }
        let flag = Arc::new(AtomicBool::new(false));
        self.listening = Some(flag.clone());
        self.note = None;
        Some(flag)
    }

    /// Show a line; returns its id, for [`Tools::clear_note`].
    pub fn set_note(&mut self, text: String, failure: bool, help: Option<Help>) -> u64 {
        self.next += 1;
        let id = self.next;
        self.note = Some(Note {
            id,
            text,
            failure,
            help,
        });
        id
    }

    /// Hide the line `id` (a newer line stays).
    pub fn clear_note(&mut self, id: u64) {
        if self.note.as_ref().is_some_and(|n| n.id == id) {
            self.note = None;
        }
    }

    /// The `@path` references of the chips.
    pub fn references(&self, cwd: Option<&Path>) -> Vec<String> {
        self.attachments
            .iter()
            .map(|a| reference(&a.path, cwd))
            .collect()
    }

    /// The text to send: the typed text and the references of the chips after it.
    pub fn message(&self, text: &str, cwd: Option<&Path>) -> String {
        let refs = self.references(cwd);
        if refs.is_empty() {
            text.to_string()
        } else if text.trim().is_empty() {
            refs.join(" ")
        } else {
            format!("{} {}", text.trim_end(), refs.join(" "))
        }
    }

    /// The composer sent its message: the chips and the line go.
    pub fn sent(&mut self) {
        self.attachments.clear();
        self.note = None;
    }
}

/// `@path` for the prompt: relative to `cwd` when the file is inside it, else absolute. A path
/// with a space gets quotes.
pub fn reference(path: &Path, cwd: Option<&Path>) -> String {
    let shown = match cwd.and_then(|c| path.strip_prefix(c).ok()) {
        Some(rel) if !rel.as_os_str().is_empty() => rel.to_string_lossy().replace('\\', "/"),
        _ => path.to_string_lossy().into_owned(),
    };
    if shown.contains(char::is_whitespace) {
        format!("@\"{shown}\"")
    } else {
        format!("@{shown}")
    }
}

/// Put `text` at the caret of the input. A space separates it from the text before it.
fn insert(input: &Entity<TextareaState>, text: &str, window: &mut Window, cx: &mut gpui::App) {
    let before = input.read(cx).value();
    let gap = !before.is_empty() && !before.ends_with(char::is_whitespace);
    let text = if gap {
        format!(" {text} ")
    } else {
        format!("{text} ")
    };
    input.update(cx, |s, cx| {
        s.insert(text, window, cx);
        s.focus(window, cx);
    });
}

/// Show a line on the tools of a screen; it hides itself.
fn note<T: 'static>(
    this: &mut T,
    access: impl Fn(&mut T) -> Option<&mut Tools> + Clone + 'static,
    text: String,
    failure: bool,
    help: Option<Help>,
    cx: &mut Context<T>,
) {
    let Some(id) = access(this).map(|tools| tools.set_note(text, failure, help)) else {
        return;
    };
    cx.spawn(async move |this, cx| {
        cx.background_executor().timer(notice::HIDE_AFTER).await;
        let _ = this.update(cx, |this, cx| {
            if let Some(tools) = access(this) {
                tools.clear_note(id);
            }
            cx.notify();
        });
    })
    .detach();
    cx.notify();
}

/// The paperclip: pick files; each becomes a chip.
pub fn attach<T: 'static>(
    access: impl Fn(&mut T) -> Option<&mut Tools> + Clone + 'static,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    let picked = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: true,
        prompt: None,
    });
    cx.spawn_in(window, async move |this, cx| {
        let paths = match picked.await {
            Ok(Ok(Some(paths))) => paths,
            Ok(Err(e)) => {
                let _ = this.update(cx, |this, cx| {
                    let text = format!("The file dialog failed. {e:#}");
                    note(this, access.clone(), text, true, None, cx);
                });
                return;
            }
            _ => return,
        };
        let _ = this.update(cx, |this, cx| {
            if let Some(tools) = access(this) {
                tools.add_files(paths, false);
            }
            cx.notify();
        });
    })
    .detach();
}

/// The folder of the captures: `<cache>/captures`.
pub fn captures_dir() -> PathBuf {
    cache_base().join("captures")
}

/// The cache folder of the app: `$AIVORY_CACHE_DIR`, else the per-user cache folder.
pub fn cache_base() -> PathBuf {
    if let Some(d) = std::env::var_os("AIVORY_CACHE_DIR") {
        PathBuf::from(d)
    } else if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA")
            .map_or_else(std::env::temp_dir, PathBuf::from)
            .join("aivory")
            .join("code")
    } else if let Some(x) = std::env::var_os("XDG_CACHE_HOME") {
        PathBuf::from(x).join("aivory-code")
    } else if cfg!(target_os = "macos") {
        std::env::home_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("Library/Caches/aivory-code")
    } else {
        std::env::home_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join(".cache/aivory-code")
    }
}

/// Capture the primary screen into a PNG in `dir`. Runs on a background thread.
pub fn capture_screen(dir: &Path) -> Result<PathBuf> {
    let monitors = xcap::Monitor::all().map_err(|e| anyhow!("no screen: {e}"))?;
    let monitor = monitors
        .iter()
        .find(|m| m.is_primary().unwrap_or(false))
        .or(monitors.first())
        .context("no screen found")?;
    let image = monitor
        .capture_image()
        .map_err(|e| anyhow!("capture failed: {e}"))?;
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let path = dir.join(format!("capture-{ms}.png"));
    image
        .save(&path)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

/// The screen capture: take a PNG of the primary screen; it becomes a chip.
pub fn capture<T: 'static>(
    access: impl Fn(&mut T) -> Option<&mut Tools> + Clone + 'static,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    let job = cx
        .background_executor()
        .spawn(async move { capture_screen(&captures_dir()) });
    cx.spawn_in(window, async move |this, cx| {
        let result = job.await;
        let _ = this.update(cx, |this, cx| {
            if let Some(tools) = access(this) {
                tools.capturing = false;
            }
            match result {
                Ok(path) => {
                    if let Some(tools) = access(this) {
                        tools.add_files(vec![path], true);
                    }
                    cx.notify();
                }
                Err(e) => {
                    let text = format!("The screen capture failed. {e:#}");
                    note(this, access.clone(), text, true, None, cx);
                }
            }
        });
    })
    .detach();
}

/// What a dictation failure means for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechProblem {
    pub text: String,
    pub help: Option<Help>,
}

/// The HRESULT in the text of an error, as `0x8004xxxx`.
pub fn hresult_in(text: &str) -> Option<u32> {
    let lower = text.to_ascii_lowercase();
    let at = lower.find("0x")?;
    let digits: String = lower[at + 2..]
        .chars()
        .take_while(char::is_ascii_hexdigit)
        .collect();
    if digits.len() == 8 {
        u32::from_str_radix(&digits, 16).ok()
    } else {
        None
    }
}

/// Words for a failure of the recognizer, and the button that opens the Windows setting.
pub fn explain_speech_error(error: &str) -> SpeechProblem {
    match hresult_in(error) {
        Some(SPEECH_POLICY_OFF) => SpeechProblem {
            text: "Turn on Online speech recognition in the Windows settings.".to_string(),
            help: Some(Help {
                label: "Open speech settings",
                url: "ms-settings:privacy-speech",
            }),
        },
        Some(ACCESS_DENIED) => SpeechProblem {
            text: "Windows blocks the microphone for this app. Allow the microphone in the Windows settings."
                .to_string(),
            help: Some(Help {
                label: "Open microphone settings",
                url: "ms-settings:privacy-microphone",
            }),
        },
        _ => SpeechProblem {
            text: format!("Dictation failed. {error}"),
            help: None,
        },
    }
}

/// The mic: listen until the speaker stops or the stop flag is set, then put the words into the
/// input. The recognizer runs on its own thread. Call [`Tools::start_listening`] first. When
/// Windows has "Online speech recognition" off, the offline model takes over, or the line offers
/// its download.
pub fn dictate<T: 'static>(
    access: impl Fn(&mut T) -> Option<&mut Tools> + Clone + 'static,
    stop: Arc<AtomicBool>,
    input: Entity<TextareaState>,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    run_dictation(
        access,
        stop,
        input,
        window,
        cx,
        |stop: &AtomicBool| recognize(stop),
        true,
    );
}

/// Run one dictation on its own thread and put the result into the composer.
fn run_dictation<T: 'static>(
    access: impl Fn(&mut T) -> Option<&mut Tools> + Clone + 'static,
    stop: Arc<AtomicBool>,
    input: Entity<TextareaState>,
    window: &mut Window,
    cx: &mut Context<T>,
    engine: impl FnOnce(&AtomicBool) -> Result<String> + Send + 'static,
    may_fall_back: bool,
) {
    let (tx, rx) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx.send(engine(&stop));
    });
    cx.spawn_in(window, async move |this, cx| {
        let result = rx
            .await
            .unwrap_or_else(|_| Err(anyhow!("the recognizer stopped")));
        let _ = this.update_in(cx, |this, window, cx| {
            if let Some(tools) = access(this) {
                tools.listening = None;
            }
            match result {
                Ok(text) if text.trim().is_empty() => {
                    note(this, access.clone(), "No speech heard.".into(), false, None, cx)
                }
                Ok(text) => {
                    insert(&input, text.trim(), window, cx);
                    cx.notify();
                }
                Err(e) => {
                    let text = format!("{e:#}");
                    let policy_off = hresult_in(&text) == Some(SPEECH_POLICY_OFF);
                    if may_fall_back && policy_off && speech::installed().is_some() {
                        note(
                            this,
                            access.clone(),
                            "Online speech recognition is off in Windows. Listening with the offline model."
                                .into(),
                            false,
                            None,
                            cx,
                        );
                        start_offline(this, access.clone(), input.clone(), window, cx);
                        return;
                    }
                    let problem = explain_speech_error(&text);
                    note(this, access.clone(), problem.text, true, problem.help, cx);
                    if policy_off && may_fall_back {
                        offer_offline(this, access.clone(), cx);
                    }
                }
            }
        });
    })
    .detach();
}

/// Dictate with the offline model. The model must be installed.
fn start_offline<T: 'static>(
    this: &mut T,
    access: impl Fn(&mut T) -> Option<&mut Tools> + Clone + 'static,
    input: Entity<TextareaState>,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    let Some(files) = speech::installed() else {
        offer_offline(this, access, cx);
        return;
    };
    let Some(stop) = access(this).and_then(Tools::start_listening) else {
        return;
    };
    cx.notify();
    run_dictation(
        access,
        stop,
        input,
        window,
        cx,
        move |stop: &AtomicBool| speech::recognize_microphone(&files, stop),
        false,
    );
}

/// Ask for the download of the offline model, with its size (read in the background).
fn offer_offline<T: 'static>(
    this: &mut T,
    access: impl Fn(&mut T) -> Option<&mut Tools> + Clone + 'static,
    cx: &mut Context<T>,
) {
    let Some(tools) = access(this) else {
        return;
    };
    if tools.offline != Offline::Idle {
        return;
    }
    tools.offline = Offline::Offer { bytes: None };
    cx.notify();
    let size = cx
        .background_executor()
        .spawn(async { speech::download_size() });
    cx.spawn(async move |this, cx| {
        let bytes = size.await;
        let _ = this.update(cx, |this, cx| {
            if let Some(tools) = access(this)
                && matches!(tools.offline, Offline::Offer { .. })
            {
                tools.offline = Offline::Offer { bytes };
                cx.notify();
            }
        });
    })
    .detach();
}

/// Download the model after the user agreed; then dictate with it.
fn start_download<T: 'static>(
    this: &mut T,
    access: impl Fn(&mut T) -> Option<&mut Tools> + Clone + 'static,
    input: Entity<TextareaState>,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    let Some(tools) = access(this) else {
        return;
    };
    let Offline::Offer { bytes } = tools.offline else {
        return;
    };
    let cancel = Arc::new(AtomicBool::new(false));
    tools.download = Some(cancel.clone());
    tools.offline = Offline::Busy(Progress::Downloading {
        done: 0,
        total: bytes,
    });
    cx.notify();
    let shared = Arc::new(std::sync::Mutex::new(Progress::Downloading {
        done: 0,
        total: bytes,
    }));
    let (tx, mut rx) = futures::channel::oneshot::channel();
    let (flag, report) = (cancel, shared.clone());
    std::thread::spawn(move || {
        let result = speech::download_and_install(bytes, &flag, &|p| {
            if let Ok(mut now) = report.lock() {
                *now = p;
            }
        });
        let _ = tx.send(result);
    });
    cx.spawn_in(window, async move |this, cx| {
        loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(300))
                .await;
            let finished = rx.try_recv();
            let progress = shared.lock().map(|p| *p).ok();
            let more = this.update_in(cx, |this, window, cx| {
                match finished {
                    Ok(None) => {
                        if let (Some(tools), Some(progress)) = (access(this), progress) {
                            tools.offline = Offline::Busy(progress);
                        }
                        cx.notify();
                        return true;
                    }
                    Ok(Some(Ok(()))) => {
                        if let Some(tools) = access(this) {
                            tools.offline = Offline::Idle;
                            tools.download = None;
                        }
                        start_offline(this, access.clone(), input.clone(), window, cx);
                    }
                    Ok(Some(Err(text))) => {
                        if let Some(tools) = access(this) {
                            tools.offline = Offline::Idle;
                            tools.download = None;
                        }
                        let cancelled = text.contains("cancelled");
                        note(this, access.clone(), text, !cancelled, None, cx);
                    }
                    Err(_) => {
                        if let Some(tools) = access(this) {
                            tools.offline = Offline::Idle;
                            tools.download = None;
                        }
                        cx.notify();
                    }
                }
                false
            });
            if !more.unwrap_or(false) {
                break;
            }
        }
    })
    .detach();
}

/// A button of the tool line was pressed. `input` is the composer that receives the words.
pub fn handle_action<T: 'static>(
    this: &mut T,
    action: ToolAction,
    access: impl Fn(&mut T) -> Option<&mut Tools> + Clone + 'static,
    input: Entity<TextareaState>,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    match action {
        ToolAction::StopListening => {
            if let Some(tools) = access(this) {
                tools.stop_listening();
            }
            cx.notify();
        }
        ToolAction::Decline => {
            if let Some(tools) = access(this) {
                tools.offline = Offline::Idle;
            }
            cx.notify();
        }
        ToolAction::CancelDownload => {
            if let Some(flag) = access(this).and_then(|t| t.download.clone()) {
                flag.store(true, Ordering::SeqCst);
            }
        }
        ToolAction::Download => start_download(this, access, input, window, cx),
    }
}

/// Dictation with the WinRT speech recognizer of Windows: the continuous session runs until the
/// speaker pauses, the stop flag is set, or 60 s pass. A stop keeps the words heard so far.
#[cfg(windows)]
fn recognize(stop: &AtomicBool) -> Result<String> {
    use std::sync::Mutex;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use windows::Foundation::TypedEventHandler;
    use windows::Media::SpeechRecognition::{
        SpeechContinuousRecognitionCompletedEventArgs,
        SpeechContinuousRecognitionResultGeneratedEventArgs, SpeechContinuousRecognitionSession,
        SpeechRecognitionResultStatus as Status, SpeechRecognitionScenario,
        SpeechRecognitionTopicConstraint, SpeechRecognizer,
    };
    use windows::core::HSTRING;

    let recognizer = SpeechRecognizer::new().context("no speech recognizer")?;
    let topic = SpeechRecognitionTopicConstraint::Create(
        SpeechRecognitionScenario::Dictation,
        &HSTRING::from("dictation"),
    )?;
    recognizer.Constraints()?.Append(&topic)?;
    let compiled = recognizer.CompileConstraintsAsync()?.join()?;
    if compiled.Status()? != Status::Success {
        return Err(anyhow!("the speech grammar did not compile"));
    }
    let session = recognizer.ContinuousRecognitionSession()?;
    let words: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink = words.clone();
    session.ResultGenerated(&TypedEventHandler::<
        SpeechContinuousRecognitionSession,
        SpeechContinuousRecognitionResultGeneratedEventArgs,
    >::new(move |_, args| {
        if let Some(args) = args.as_ref() {
            let result = args.Result()?;
            if result.Status()? == Status::Success {
                let text = result.Text()?.to_string();
                if !text.trim().is_empty()
                    && let Ok(mut all) = sink.lock()
                {
                    all.push(text);
                }
            }
        }
        Ok(())
    }))?;
    let (done_tx, done_rx) = mpsc::channel::<()>();
    session.Completed(&TypedEventHandler::<
        SpeechContinuousRecognitionSession,
        SpeechContinuousRecognitionCompletedEventArgs,
    >::new(move |_, _| {
        let _ = done_tx.send(());
        Ok(())
    }))?;
    session.StartAsync()?.join()?;
    let started = Instant::now();
    while !stop.load(Ordering::SeqCst)
        && started.elapsed() < Duration::from_secs(60)
        && done_rx.recv_timeout(Duration::from_millis(100)).is_err()
    {}
    // A session that ended on its own refuses to stop: the words are kept either way.
    if let Ok(op) = session.StopAsync() {
        let _ = op.join();
    }
    let all = words.lock().map_err(|_| anyhow!("the recognizer failed"))?;
    Ok(all.join(" "))
}

#[cfg(not(windows))]
fn recognize(_stop: &AtomicBool) -> Result<String> {
    Err(anyhow!("dictation needs Windows"))
}

// ---- the lines over the input ---------------------------------------------------------------

/// The colors of the tool lines (`0xRRGGBBAA`): a console pane, or the light screens.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub text: u32,
    pub dim: u32,
    pub line: u32,
    pub chip_bg: u32,
    pub hover: u32,
    pub fail: u32,
    pub ok: u32,
}

impl Palette {
    /// The palette of the page (the current look).
    pub fn page() -> Self {
        let tk = tk();
        Self {
            text: (tk.ink << 8) | 0xFF,
            dim: (tk.ink_3 << 8) | 0xFF,
            line: tk.line_2,
            chip_bg: (tk.track << 8) | 0xFF,
            hover: (tk.track_2 << 8) | 0xFF,
            fail: (tk.e_text << 8) | 0xFF,
            ok: (tk.ok << 8) | 0xFF,
        }
    }

    /// The palette of a console theme.
    pub fn of(t: &PaneTheme) -> Self {
        Self {
            text: t.text,
            dim: t.dim,
            line: t.field_border,
            chip_bg: t.code_bg,
            hover: t.ghost_hover,
            fail: t.del,
            ok: t.tool,
        }
    }
}

/// The chips of the attachments. `remove(i)` builds the click handler of the cross of chip `i`.
pub fn chips(
    tools: &Tools,
    id: &str,
    p: Palette,
    mut remove: impl FnMut(usize) -> OnClick,
) -> Option<Div> {
    if tools.attachments.is_empty() {
        return None;
    }
    let mut row = div().flex().flex_wrap().gap(px(6.)).min_w(px(0.));
    for (i, a) in tools.attachments.iter().enumerate() {
        row = row.child(
            div()
                .h(px(22.))
                .max_w(px(220.))
                .pl(px(8.))
                .pr(px(2.))
                .flex()
                .items_center()
                .gap(px(4.))
                .rounded(px(4.))
                .border_1()
                .border_color(rgba(p.line))
                .bg(rgba(p.chip_bg))
                .text_size(crate::ui::ts(crate::theme::T_SMALL))
                .text_color(rgba(p.text))
                .child(ui::icon(
                    if a.capture {
                        "icons/capture.svg"
                    } else {
                        "icons/attach.svg"
                    },
                    12.,
                    gpui::rgba(p.dim),
                ))
                .child(
                    div()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(a.name.clone()),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("chip-x-{id}-{i}")))
                        .size(px(18.))
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .rounded(px(3.))
                        .text_color(rgba(p.dim))
                        .cursor_pointer()
                        .hover(move |s| s.bg(rgba(p.hover)).text_color(rgba(p.text)))
                        .tooltip(ui::tip(format!("Remove {}", a.name)))
                        .on_click(remove(i))
                        .child("×"),
                ),
        );
    }
    Some(row)
}

/// The words that ask for the download of the offline model.
pub fn offer_text(bytes: Option<u64>) -> String {
    match bytes {
        Some(b) => format!(
            "Download the offline speech model (~{} MB)?",
            speech::megabytes(b)
        ),
        None => "Download the offline speech model?".to_string(),
    }
}

/// The state lines: "Listening…" with a stop button, "Capturing…", the note of the last tool
/// with the button that opens the setting that fixes it, and the offer of the offline model.
/// `act(action)` builds the click handler of a button.
pub fn status(
    tools: &Tools,
    id: &str,
    p: Palette,
    mut act: impl FnMut(ToolAction) -> OnClick,
) -> Option<Div> {
    let button = |name: &str, label: &'static str, on_click: OnClick| {
        div()
            .id(SharedString::from(format!("{name}-{id}")))
            .h(px(22.))
            .px(px(8.))
            .flex()
            .flex_none()
            .items_center()
            .rounded(px(4.))
            .border_1()
            .border_color(rgba(p.line))
            .text_color(rgba(p.text))
            .whitespace_nowrap()
            .cursor_pointer()
            .hover(move |s| s.bg(rgba(p.hover)))
            .on_click(on_click)
            .child(label)
    };
    let row = || {
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .min_w(px(0.))
            .text_size(crate::ui::ts(crate::theme::T_SMALL))
            .text_color(rgba(p.dim))
    };
    let dot = |color: u32| {
        div()
            .size(px(8.))
            .flex_none()
            .rounded_full()
            .bg(rgba(color))
    };
    let mut lines: Vec<Div> = Vec::new();
    if tools.listening.is_some() {
        lines.push(
            row()
                .child(dot(p.fail))
                .child(
                    div()
                        .text_color(rgba(p.text))
                        .font_weight(FontWeight::MEDIUM)
                        .child("Listening… speak now"),
                )
                .child(button("mic-stop", "Stop", act(ToolAction::StopListening))),
        );
    } else if tools.capturing {
        lines.push(row().child("Capturing the screen…"));
    } else if let Some(note) = tools.note.as_ref() {
        let mut line = row()
            .child(dot(if note.failure { p.fail } else { p.ok }))
            .child(
                div()
                    .min_w(px(0.))
                    .text_color(rgba(p.text))
                    .child(note.text.clone()),
            );
        if let Some(help) = note.help.clone() {
            line = line.child(button(
                "note-help",
                help.label,
                Box::new(move |_, _, cx| cx.open_url(help.url)),
            ));
        }
        lines.push(line);
    }
    match tools.offline {
        Offline::Idle => {}
        Offline::Offer { bytes } => lines.push(
            row()
                .child(
                    div()
                        .min_w(px(0.))
                        .text_color(rgba(p.text))
                        .child(offer_text(bytes)),
                )
                .child(button("offline-yes", "Download", act(ToolAction::Download)))
                .child(button("offline-no", "Not now", act(ToolAction::Decline))),
        ),
        Offline::Busy(progress) => {
            let mut line = row().child(
                div()
                    .min_w(px(0.))
                    .text_color(rgba(p.text))
                    .child(speech::progress_text(progress)),
            );
            if matches!(progress, Progress::Downloading { .. }) {
                line = line.child(button(
                    "offline-cancel",
                    "Cancel",
                    act(ToolAction::CancelDownload),
                ));
            }
            lines.push(line);
        }
    }
    if lines.is_empty() {
        return None;
    }
    Some(div().flex().flex_col().gap(px(4.)).children(lines))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_inside_the_session_folder_is_relative() {
        let cwd = Path::new("C:/work/shop");
        let inside = Path::new("C:/work/shop/src/pay.rs");
        assert_eq!(reference(inside, Some(cwd)), "@src/pay.rs");
        let outside = Path::new("C:/other/a.png");
        assert_eq!(reference(outside, Some(cwd)), "@C:/other/a.png");
        assert_eq!(reference(outside, None), "@C:/other/a.png");
    }

    #[test]
    fn a_path_with_a_space_has_quotes() {
        let path = Path::new("C:/my docs/a.txt");
        assert_eq!(reference(path, None), "@\"C:/my docs/a.txt\"");
    }

    #[test]
    fn captures_go_to_the_cache_folder() {
        assert!(captures_dir().ends_with("captures"));
    }

    #[test]
    fn online_speech_off_has_words_and_a_settings_button() {
        let p = explain_speech_error(
            "Speech privacy policy was not accepted prior to attempting a speech recognition (0x80045509)",
        );
        assert!(p.text.starts_with("Turn on Online speech recognition"));
        assert_eq!(p.help.unwrap().url, "ms-settings:privacy-speech");
    }

    #[test]
    fn a_blocked_microphone_opens_the_microphone_page() {
        let p = explain_speech_error("Access is denied. (0x80070005)");
        assert_eq!(p.help.unwrap().url, "ms-settings:privacy-microphone");
    }

    #[test]
    fn another_failure_keeps_its_text_and_has_no_button() {
        let p = explain_speech_error("the speech grammar did not compile");
        assert_eq!(
            p.text,
            "Dictation failed. the speech grammar did not compile"
        );
        assert!(p.help.is_none());
        assert_eq!(hresult_in("code 0x12"), None);
        assert_eq!(hresult_in("(0X80045509)"), Some(SPEECH_POLICY_OFF));
    }

    #[test]
    fn chips_add_once_and_remove_by_index() {
        let mut t = Tools::default();
        t.add_files(vec!["C:/a/x.png".into(), "C:/a/x.png".into()], true);
        t.add_files(vec!["C:/a/y.rs".into()], false);
        assert_eq!(t.attachments.len(), 2);
        assert_eq!(t.attachments[0].name, "x.png");
        assert!(t.attachments[0].capture && !t.attachments[1].capture);
        assert!(t.active());
        t.remove(0);
        assert_eq!(t.attachments[0].name, "y.rs");
        t.remove(9);
        assert_eq!(t.attachments.len(), 1);
    }

    #[test]
    fn the_message_ends_with_the_references_of_the_chips() {
        let mut t = Tools::default();
        let cwd = Path::new("C:/work/shop");
        assert_eq!(t.message("fix it", Some(cwd)), "fix it");
        t.add_files(
            vec!["C:/work/shop/src/pay.rs".into(), "C:/o/a b.png".into()],
            false,
        );
        assert_eq!(
            t.message("fix it ", Some(cwd)),
            "fix it @src/pay.rs @\"C:/o/a b.png\""
        );
        assert_eq!(t.message("", Some(cwd)), "@src/pay.rs @\"C:/o/a b.png\"");
        t.sent();
        assert!(!t.active());
    }

    #[test]
    fn the_offer_names_the_size() {
        assert_eq!(
            offer_text(Some(127_887_156)),
            "Download the offline speech model (~128 MB)?"
        );
        assert_eq!(offer_text(None), "Download the offline speech model?");
    }

    #[test]
    fn an_offer_or_a_download_keeps_the_composer_open() {
        let mut t = Tools::default();
        assert!(!t.active());
        t.offline = Offline::Offer { bytes: None };
        assert!(t.active());
        t.offline = Offline::Busy(Progress::Unpacking);
        assert!(t.active());
        t.offline = Offline::Idle;
        assert!(!t.active());
    }

    #[test]
    fn the_composer_stays_open_while_a_tool_runs_or_a_line_shows() {
        let mut t = Tools::default();
        assert!(!t.active());
        let stop = t.start_listening().expect("starts");
        assert!(t.busy() && t.active());
        assert!(t.start_listening().is_none());
        assert!(t.stop_listening());
        assert!(stop.load(Ordering::SeqCst));
        t.listening = None;
        assert!(!t.stop_listening());
        let id = t.set_note("x".into(), true, None);
        assert!(t.active());
        t.clear_note(id + 1);
        assert!(t.note.is_some());
        t.clear_note(id);
        assert!(!t.active());
    }

    /// Needs a microphone and the speech settings of Windows: `cargo test -- --ignored`.
    #[test]
    #[ignore = "needs a microphone"]
    fn the_recognizer_starts() {
        let stop = AtomicBool::new(true);
        let result = recognize(&stop);
        eprintln!("recognize: {result:?}");
    }
}
