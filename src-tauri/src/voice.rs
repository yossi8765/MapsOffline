//! Spoken instructions through Windows text-to-speech (WinRT `SpeechSynthesizer`, played with
//! `MediaPlayer`): a Hebrew voice ("Microsoft Asaf", he-IL) when installed, else the default
//! voice. No download, nothing leaves the machine. One worker thread owns the synthesizer and
//! the player; a new sentence interrupts the previous one.

use std::sync::mpsc::{channel, Sender};

use parking_lot::Mutex;

enum Msg {
    Say(String),
    Silence,
}

static WORKER: Mutex<Option<Sender<Msg>>> = Mutex::new(None);

fn worker() -> Option<Sender<Msg>> {
    let mut g = WORKER.lock();
    if g.is_none() {
        let (tx, rx) = channel::<Msg>();
        let ok = std::thread::Builder::new()
            .name("voice".into())
            .spawn(move || {
                init_winrt_thread();
                let mut engine = match imp::Engine::new() {
                    Ok(e) => Some(e),
                    Err(e) => {
                        log::warn!("text-to-speech unavailable: {e}");
                        None
                    }
                };
                while let Ok(m) = rx.recv() {
                    let Some(en) = engine.as_mut() else { continue };
                    let r = match m {
                        Msg::Say(t) => en.say(&t),
                        Msg::Silence => en.silence(),
                    };
                    if let Err(e) = r {
                        log::warn!("speech: {e}");
                    }
                }
            })
            .is_ok();
        if ok {
            *g = Some(tx);
        }
    }
    g.clone()
}

/// Speak `text` (interrupts what is being said).
pub fn say(text: &str) {
    if let Some(tx) = worker() {
        let _ = tx.send(Msg::Say(text.to_string()));
    }
}

/// Stop speaking.
pub fn silence() {
    if let Some(tx) = WORKER.lock().as_ref() {
        let _ = tx.send(Msg::Silence);
    }
}

/// Installed voices: (display name, language tag), the chosen one first.
pub fn voices() -> Vec<(String, String)> {
    imp::voices().unwrap_or_default()
}

/// WinRT objects need an initialised apartment on the calling thread (MTA here).
pub fn init_winrt_thread() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }
}

#[cfg(windows)]
mod imp {
    use windows::core::{Interface, HSTRING};
    use windows::Media::Core::MediaSource;
    use windows::Media::Playback::MediaPlayer;
    use windows::Media::SpeechSynthesis::{SpeechSynthesizer, VoiceInformation};
    use windows::Storage::Streams::IRandomAccessStream;

    pub struct Engine {
        synth: SpeechSynthesizer,
        player: MediaPlayer,
    }

    /// he-IL first ("Microsoft Asaf"), else the system default voice.
    fn hebrew_voice() -> windows::core::Result<Option<VoiceInformation>> {
        let all = SpeechSynthesizer::AllVoices()?;
        for v in &all {
            if v.Language()?.to_string().to_ascii_lowercase().starts_with("he") {
                return Ok(Some(v));
            }
        }
        Ok(None)
    }

    impl Engine {
        pub fn new() -> windows::core::Result<Self> {
            let synth = SpeechSynthesizer::new()?;
            if let Some(v) = hebrew_voice()? {
                synth.SetVoice(&v)?;
            }
            let player = MediaPlayer::new()?;
            Ok(Self { synth, player })
        }

        pub fn say(&mut self, text: &str) -> windows::core::Result<()> {
            let stream = self.synth.SynthesizeTextToStreamAsync(&HSTRING::from(text))?.get()?;
            let ct = stream.ContentType()?;
            let ras: IRandomAccessStream = stream.cast()?;
            let src = MediaSource::CreateFromStream(&ras, &ct)?;
            self.player.SetSource(&src)?;
            self.player.Play()
        }

        pub fn silence(&mut self) -> windows::core::Result<()> {
            self.player.Pause()
        }
    }

    pub fn voices() -> windows::core::Result<Vec<(String, String)>> {
        super::init_winrt_thread();
        let mut out = Vec::new();
        let chosen = hebrew_voice()?.map(|v| v.Id().map(|s| s.to_string()).unwrap_or_default());
        for v in &SpeechSynthesizer::AllVoices()? {
            let item = (v.DisplayName()?.to_string(), v.Language()?.to_string());
            if chosen.as_deref() == Some(v.Id()?.to_string().as_str()) {
                out.insert(0, item);
            } else {
                out.push(item);
            }
        }
        Ok(out)
    }
}

/// Linux: speech-dispatcher (`spd-say`, the desktop's speech service) or espeak-ng, in Hebrew.
/// A new sentence stops the one still being spoken.
#[cfg(target_os = "linux")]
mod imp {
    use std::process::{Child, Command, Stdio};

    fn has(cmd: &str) -> bool {
        Command::new("sh").args(["-c", &format!("command -v {cmd}")]).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
    }

    pub struct Engine {
        espeak: bool,
        child: Option<Child>,
    }
    impl Engine {
        pub fn new() -> anyhow::Result<Self> {
            if has("espeak-ng") {
                Ok(Engine { espeak: true, child: None })
            } else if has("spd-say") {
                Ok(Engine { espeak: false, child: None })
            } else {
                anyhow::bail!("no espeak-ng / spd-say (install espeak-ng for spoken instructions)")
            }
        }
        pub fn say(&mut self, t: &str) -> anyhow::Result<()> {
            self.silence()?;
            let child = if self.espeak {
                Command::new("espeak-ng").args(["-v", "he", "-s", "165", "--", t]).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?
            } else {
                Command::new("spd-say").args(["-l", "he", "-w", "--", t]).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?
            };
            self.child = Some(child);
            Ok(())
        }
        pub fn silence(&mut self) -> anyhow::Result<()> {
            if let Some(mut c) = self.child.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
            if !self.espeak {
                let _ = Command::new("spd-say").arg("-C").status();
            }
            Ok(())
        }
    }
    pub fn voices() -> anyhow::Result<Vec<(String, String)>> {
        Ok(if has("espeak-ng") {
            vec![("eSpeak NG".to_string(), "he".to_string())]
        } else if has("spd-say") {
            vec![("Speech Dispatcher".to_string(), "he".to_string())]
        } else {
            Vec::new()
        })
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod imp {
    pub struct Engine;
    impl Engine {
        pub fn new() -> anyhow::Result<Self> {
            anyhow::bail!("no text-to-speech on this platform")
        }
        pub fn say(&mut self, _t: &str) -> anyhow::Result<()> {
            Ok(())
        }
        pub fn silence(&mut self) -> anyhow::Result<()> {
            Ok(())
        }
    }
    pub fn voices() -> anyhow::Result<Vec<(String, String)>> {
        Ok(Vec::new())
    }
}

#[cfg(all(test, windows))]
mod tests {
    /// Synthesises a Hebrew sentence with the installed voice (no playback) and lists the voices.
    /// `cargo test --lib voice -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn hebrew_voice_synthesises() {
        use windows::core::HSTRING;
        use windows::Media::SpeechSynthesis::SpeechSynthesizer;
        super::init_winrt_thread();
        let v = super::voices();
        println!("voices: {v:?}");
        assert!(!v.is_empty());
        let mut e = super::imp::Engine::new().unwrap();
        let synth = SpeechSynthesizer::new().unwrap();
        let s = synth.SynthesizeTextToStreamAsync(&HSTRING::from("בעוד 150 מטר, פנה ימינה לרחוב הרצל")).unwrap().get().unwrap();
        let size = s.Size().unwrap();
        println!("voice {} – {size} bytes of audio", synth.Voice().unwrap().DisplayName().unwrap());
        assert!(size > 10_000);
        e.say("בדיקה").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1500));
    }
}
