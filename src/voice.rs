//! Reading aloud. A voice is a client of the reader like any other: it
//! takes the book's sentences with `text.read`, says each in turn, shows
//! which with `voice.progress`, and turns the page with `reader.turn` when
//! the next sentence is over it. The reader starts it (`voice.start`) as a
//! process of its own, and stops it by ending that process.
//!
//! What says the sentences is an engine, named under `[voice.engines]` in
//! config.toml or built in:
//!
//! - `system`: the system's own speech (`say` on macOS; `espeak-ng` or
//!   `spd-say` elsewhere)
//! - `voiceover`: hands the open pages to VoiceOver whenever they change,
//!   and leaves the reading and the turning to the reader
//! - `openai`: OpenAI's speech API, or one in its shape (`url`)
//! - `command`: any program, given each sentence on stdin (or in place of
//!   `{}` among its arguments)

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::api::{
    self, Call, EmptyParams, EventData, Incoming, ResponseResult, SentenceInfo, Subscription,
    TextReadParams, VoiceProgressParams, VoiceStopParams,
};
use crate::cli::CliError;
use crate::client::Client;
use crate::doc::Kind;
use crate::layout::Pos;
use crate::spoken::RubyReading;

/// The engine used when none is named here or in config.toml.
pub const DEFAULT_ENGINE: &str = "system";
/// Pages of sentences asked for at a time.
const AHEAD: usize = 4;

/// What says the sentences.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Engine {
    System {
        /// The system's name for the voice (`say -v '?'` lists them on macOS).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        voice: Option<String>,
        /// Words a minute.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rate: Option<u32>,
    },
    Voiceover,
    Command {
        command: Vec<String>,
    },
    Openai {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        voice: Option<String>,
        /// How to speak: "calm, as if reading a novel aloud".
        #[serde(default, skip_serializing_if = "Option::is_none")]
        instructions: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        speed: Option<f32>,
        /// Another server in the same shape.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        /// The environment variable holding the key.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key_env: Option<String>,
        /// The sound asked for: `mp3`, `wav`, `aac`, `flac`, `opus`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<String>,
        /// What plays the sound it returns, the file's path last.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        player: Option<Vec<String>>,
    },
}

/// What is read of code: nothing, or each line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    #[default]
    Skip,
    Read,
}

/// The `[voice]` table of config.toml.
#[derive(Debug, Default, Deserialize)]
struct Config {
    engine: Option<String>,
    #[serde(default)]
    code: Code,
    #[serde(default)]
    ruby: RubyReading,
    #[serde(default)]
    engines: BTreeMap<String, Engine>,
}

/// A voice ready to read: its engine and how it reads.
#[derive(Clone, Debug)]
pub struct Voice {
    pub name: String,
    pub engine: Engine,
    pub code: Code,
    pub ruby: RubyReading,
}

fn built_in() -> BTreeMap<String, Engine> {
    BTreeMap::from([
        (
            "system".to_string(),
            Engine::System {
                voice: None,
                rate: None,
            },
        ),
        ("voiceover".to_string(), Engine::Voiceover),
        (
            "openai".to_string(),
            Engine::Openai {
                model: None,
                voice: None,
                instructions: None,
                speed: None,
                url: None,
                key_env: None,
                format: None,
                player: None,
            },
        ),
    ])
}

fn config() -> Result<Config, CliError> {
    let Some(path) = crate::keys::config_path() else {
        return Ok(Config::default());
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(e) => return Err(CliError::io(e)),
    };
    #[derive(Deserialize)]
    struct File {
        #[serde(default)]
        voice: Option<Config>,
    }
    let file: File = toml::from_str(&text)
        .map_err(|e| CliError::new("invalid_config", format!("[voice]: {}", e.message())))?;
    Ok(file.voice.unwrap_or_default())
}

/// The engines there are, by name: those built in, and those config.toml
/// names (which may take a built-in name over).
fn engines(config: &Config) -> BTreeMap<String, Engine> {
    let mut all = built_in();
    all.extend(config.engines.clone());
    all
}

/// The voice called `name`, or the one config.toml names, or `system`.
pub fn resolve(name: Option<&str>) -> Result<Voice, CliError> {
    let config = config()?;
    let name = name
        .map(str::to_string)
        .or(config.engine.clone())
        .unwrap_or_else(|| DEFAULT_ENGINE.to_string());
    let all = engines(&config);
    let engine = all.get(&name).cloned().ok_or_else(|| {
        let names: Vec<&str> = all.keys().map(String::as_str).collect();
        CliError::new(
            "unknown_engine",
            format!("no voice {name:?}; there are: {}", names.join(", ")),
        )
    })?;
    Ok(Voice {
        name,
        engine,
        code: config.code,
        ruby: config.ruby,
    })
}

/// `herdfold voice engines`: the engines there are, and the one used when
/// none is named.
pub fn list() -> Result<(), CliError> {
    let config = config()?;
    let default = config
        .engine
        .clone()
        .unwrap_or_else(|| DEFAULT_ENGINE.to_string());
    let engines: Vec<serde_json::Value> = engines(&config)
        .into_iter()
        .map(|(name, engine)| {
            let mut v = serde_json::to_value(&engine).unwrap_or_default();
            v["name"] = name.into();
            v
        })
        .collect();
    println!(
        "{}",
        serde_json::json!({
            "id": "cli:voice:engines",
            "result": { "type": "voice_engines", "engines": engines, "default": default },
        })
    );
    Ok(())
}

/// `herdfold voice say TEXT`: says `text`, to try a voice.
pub fn say(name: Option<String>, text: String) -> Result<(), CliError> {
    let voice = resolve(name.as_deref())?;
    let fail = |e: String| CliError::new("voice_failed", e);
    if voice.engine == Engine::Voiceover {
        return voiceover(&text).map_err(fail);
    }
    let clip = voice.engine.prepare(&text, 0).map_err(fail)?;
    let (_tx, heard) = mpsc::channel();
    play(clip, &heard, &mut Vec::new()).map_err(fail)?;
    Ok(())
}

/// Something heard from the reader while reading.
enum Heard {
    /// The open pages begin here now.
    Moved(Pos),
    Closed,
}

/// How saying a sentence ended.
enum Said {
    Done,
    /// The reader went elsewhere: read on from there.
    Moved(Pos),
    Closed,
}

/// `herdfold voice run`: reads the book at the reader aloud with the voice
/// called `name`, from `from` (else the open pages), until the book ends,
/// the reader closes, or the process is ended.
pub fn run(name: Option<String>, from: Option<Pos>) -> Result<(), CliError> {
    let voice = resolve(name.as_deref())?;
    let mut client = Client::connect("voice")?;
    let mut listen = Client::connect("voice-events")?;
    listen.call(Call::EventsSubscribe(api::EventsSubscribeParams {
        subscriptions: vec![Subscription::ReaderMoved, Subscription::ReaderClosed],
    }))?;
    let (tx, heard) = mpsc::channel();
    thread::spawn(move || {
        for msg in listen.incoming.iter() {
            let h = match msg {
                Incoming::Event(e) => match e.data {
                    EventData::ReaderMoved { at, .. } => Heard::Moved(at.pos()),
                    EventData::ReaderClosed => Heard::Closed,
                    _ => continue,
                },
                _ => continue,
            };
            if tx.send(h).is_err() {
                break;
            }
        }
        let _ = tx.send(Heard::Closed);
    });
    let result = match voice.engine {
        Engine::Voiceover => announce(&voice, &mut client, &heard),
        _ => read_aloud(&voice, &mut client, &heard, from),
    };
    // The reader shows why the voice stopped, if it could not go on.
    let _ = client.call(Call::VoiceStop(VoiceStopParams {
        error: result.as_ref().err().cloned(),
    }));
    result.map_err(|e| CliError::new("voice_failed", e))
}

fn state(client: &mut Client) -> Result<api::ReaderStateInfo, String> {
    match client
        .call(Call::ReaderState(EmptyParams {}))
        .map_err(|e| e.message)?
    {
        ResponseResult::ReaderState { state } => Ok(state),
        other => Err(format!("unexpected answer: {other:?}")),
    }
}

fn text(
    client: &mut Client,
    from: Pos,
    pages: usize,
    voice: &Voice,
) -> Result<(Vec<SentenceInfo>, Option<Pos>), String> {
    let answer = client
        .call(Call::TextRead(TextReadParams {
            from: Some(from),
            pages: Some(pages),
            ruby: voice.ruby,
        }))
        .map_err(|e| e.message)?;
    let ResponseResult::Text { sentences, next } = answer else {
        return Err(format!("unexpected answer: {answer:?}"));
    };
    let sentences = sentences
        .into_iter()
        .filter(|s| voice.code == Code::Read || s.kind != Kind::Pre)
        .collect();
    Ok((sentences, next))
}

/// Whether the reader has gone elsewhere (or away) since the voice last
/// looked, its own turns aside.
fn moved(heard: &Receiver<Heard>, ours: &mut Vec<Pos>) -> Option<Said> {
    while let Ok(h) = heard.try_recv() {
        match h {
            Heard::Moved(at) => {
                if let Some(i) = ours.iter().position(|&p| p == at) {
                    ours.drain(..=i);
                } else {
                    return Some(Said::Moved(at));
                }
            }
            Heard::Closed => return Some(Said::Closed),
        }
    }
    None
}

fn read_aloud(
    voice: &Voice,
    client: &mut Client,
    heard: &Receiver<Heard>,
    from: Option<Pos>,
) -> Result<(), String> {
    let mut pos = match from {
        Some(at) => at,
        None => state(client)?.at.pos(),
    };
    // Where the voice's own turns lead, so they are not taken for the
    // reader going elsewhere.
    let mut ours: Vec<Pos> = Vec::new();
    loop {
        let (sentences, next) = text(client, pos, AHEAD, voice)?;
        let clips = prepare_ahead(
            voice.engine.clone(),
            sentences.iter().map(|s| s.text.clone()).collect(),
        );
        let mut went = None;
        for s in &sentences {
            let clip = clips
                .recv()
                .map_err(|_| "the voice stopped".to_string())??;
            let said = match turn_to(client, s.page, heard, &mut ours)? {
                Some(said) => {
                    clip.discard();
                    said
                }
                None => {
                    client
                        .call(Call::VoiceProgress(VoiceProgressParams {
                            at: s.at,
                            end: s.end,
                        }))
                        .map_err(|e| e.message)?;
                    play(clip, heard, &mut ours)?
                }
            };
            match said {
                Said::Done => {}
                Said::Moved(at) => {
                    went = Some(at);
                    break;
                }
                Said::Closed => return Ok(()),
            }
        }
        // Sounds made ahead for sentences now not read.
        for clip in clips.try_iter().flatten() {
            clip.discard();
        }
        pos = match (went, next) {
            (Some(at), _) => at,
            (None, Some(next)) => next,
            (None, None) => return Ok(()),
        };
    }
}

/// Turns to the spread with `page` on it, if it is the next one; if the
/// reader has gone somewhere else, says so.
fn turn_to(
    client: &mut Client,
    page: usize,
    heard: &Receiver<Heard>,
    ours: &mut Vec<Pos>,
) -> Result<Option<Said>, String> {
    if let Some(said) = moved(heard, ours) {
        return Ok(Some(said));
    }
    let state = state(client)?;
    if state.open.contains(&page) {
        return Ok(None);
    }
    let last = state.open.iter().max().copied().unwrap_or(0);
    if page <= last || page > last + state.open.len() {
        return Ok(Some(Said::Moved(state.at.pos())));
    }
    let answer = client
        .call(Call::ReaderTurn(api::ReaderTurnParams {
            direction: api::TurnDirection::Forward,
        }))
        .map_err(|e| e.message)?;
    if let ResponseResult::Moved { at } = answer {
        ours.push(at.pos());
    }
    Ok(None)
}

/// For a screen reader: the open pages, whenever they change.
fn announce(voice: &Voice, client: &mut Client, heard: &Receiver<Heard>) -> Result<(), String> {
    loop {
        let state = state(client)?;
        let (sentences, _) = text(client, state.at.pos(), state.open.len().max(1), voice)?;
        let all: Vec<&str> = sentences.iter().map(|s| s.text.as_str()).collect();
        voiceover(&all.join(" "))?;
        match heard.recv() {
            Ok(Heard::Moved(_)) => {}
            Ok(Heard::Closed) | Err(_) => return Ok(()),
        }
    }
}

/// Has VoiceOver say `text`, as it says what it finds on the screen.
fn voiceover(text: &str) -> Result<(), String> {
    const WAIT: Duration = Duration::from_secs(10);
    let mut child = Command::new("osascript")
        .args([
            "-e",
            "on run argv",
            "-e",
            "tell application \"VoiceOver\" to output (item 1 of argv)",
            "-e",
            "end run",
            text,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run osascript: {e}"))?;
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if started.elapsed() > WAIT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(
                "VoiceOver did not answer; macOS may be asking whether this \
                        terminal may control VoiceOver (System Settings → Privacy & \
                        Security → Automation)"
                    .into(),
            );
        }
        thread::sleep(Duration::from_millis(50));
    };
    if status.success() {
        return Ok(());
    }
    let mut err = String::new();
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_string(&mut err);
    }
    Err(format!(
        "VoiceOver did not take it ({}); turn VoiceOver on, and allow it to be \
         controlled with AppleScript in VoiceOver Utility",
        err.trim()
    ))
}

/// A sentence ready to be said.
enum Clip {
    /// A program that says it.
    Run {
        argv: Vec<String>,
        input: Option<String>,
    },
    /// A sound made for it, and what plays it.
    Sound { path: PathBuf, player: Vec<String> },
}

impl Clip {
    /// Lets go of a clip not played.
    fn discard(self) {
        if let Clip::Sound { path, .. } = self {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Makes the clips for `texts` in order, a little ahead of their playing,
/// so a voice that takes time to make sound does not pause between them.
fn prepare_ahead(engine: Engine, texts: Vec<String>) -> Receiver<Result<Clip, String>> {
    let (tx, rx): (SyncSender<Result<Clip, String>>, _) = mpsc::sync_channel(1);
    thread::spawn(move || {
        for (n, text) in texts.iter().enumerate() {
            match tx.send(engine.prepare(text, n)) {
                Ok(()) => {}
                Err(e) => {
                    if let Ok(clip) = e.0 {
                        clip.discard();
                    }
                    break;
                }
            }
        }
    });
    rx
}

impl Engine {
    /// The clip that says `text`, the `n`th of its run.
    fn prepare(&self, text: &str, n: usize) -> Result<Clip, String> {
        match self {
            Engine::System { voice, rate } => system(voice.as_deref(), *rate, text),
            Engine::Command { command } => {
                let (program, args) = command
                    .split_first()
                    .ok_or("a command voice needs `command`")?;
                let placed = args.iter().any(|a| a.contains("{}"));
                let argv = std::iter::once(program.clone())
                    .chain(args.iter().map(|a| a.replace("{}", text)))
                    .collect();
                Ok(Clip::Run {
                    argv,
                    input: (!placed).then(|| text.to_string()),
                })
            }
            Engine::Openai {
                model,
                voice,
                instructions,
                speed,
                url,
                key_env,
                format,
                player,
            } => {
                let format = format.as_deref().unwrap_or("mp3");
                let key_env = key_env.as_deref().unwrap_or("OPENAI_API_KEY");
                let key = std::env::var(key_env)
                    .map_err(|_| format!("set {key_env} to the API key for the openai voice"))?;
                let mut body = serde_json::json!({
                    "model": model.as_deref().unwrap_or("gpt-4o-mini-tts"),
                    "voice": voice.as_deref().unwrap_or("alloy"),
                    "input": text,
                    "response_format": format,
                });
                if let Some(i) = instructions {
                    body["instructions"] = i.as_str().into();
                }
                if let Some(s) = speed {
                    body["speed"] = (*s).into();
                }
                let url = url
                    .as_deref()
                    .unwrap_or("https://api.openai.com/v1/audio/speech");
                let sound = speech(url, &key, &body)?;
                let path = std::env::temp_dir().join(format!(
                    "herdfold-voice-{}-{n}.{format}",
                    std::process::id()
                ));
                std::fs::write(&path, sound).map_err(|e| e.to_string())?;
                let player = match player {
                    Some(p) if !p.is_empty() => p.clone(),
                    _ => default_player()?,
                };
                Ok(Clip::Sound { path, player })
            }
            Engine::Voiceover => Err("VoiceOver reads the pages itself".into()),
        }
    }
}

/// Asks a speech API in OpenAI's shape for `body` said.
fn speech(url: &str, key: &str, body: &serde_json::Value) -> Result<Vec<u8>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(60)))
        .build()
        .into();
    let mut res = agent
        .post(url)
        .header("Authorization", &format!("Bearer {key}"))
        .send_json(body)
        .map_err(|e| format!("{url}: {e}"))?;
    let status = res.status();
    let bytes = res
        .body_mut()
        .with_config()
        .limit(64 * 1024 * 1024)
        .read_to_vec()
        .map_err(|e| format!("{url}: {e}"))?;
    if !status.is_success() {
        // The API says what was wrong in `error.message`.
        let said = serde_json::from_slice::<serde_json::Value>(&bytes)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
            .unwrap_or_else(|| String::from_utf8_lossy(&bytes).into_owned());
        return Err(format!("{url} answered {status}: {said}"));
    }
    Ok(bytes)
}

/// Whether `program` is on the PATH.
fn found(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

/// The system's own speech, saying `text`.
fn system(voice: Option<&str>, rate: Option<u32>, text: &str) -> Result<Clip, String> {
    let mut argv: Vec<String>;
    let mut input = Some(text.to_string());
    if cfg!(target_os = "macos") {
        argv = vec!["say".into()];
        if let Some(v) = voice {
            argv.extend(["-v".into(), v.into()]);
        }
        if let Some(r) = rate {
            argv.extend(["-r".into(), r.to_string()]);
        }
    } else if found("espeak-ng") {
        argv = vec!["espeak-ng".into(), "--stdin".into()];
        if let Some(v) = voice {
            argv.extend(["-v".into(), v.into()]);
        }
        if let Some(r) = rate {
            argv.extend(["-s".into(), r.to_string()]);
        }
    } else if found("spd-say") {
        argv = vec!["spd-say".into(), "--wait".into()];
        if let Some(v) = voice {
            argv.extend(["--synthesis-voice".into(), v.into()]);
        }
        argv.extend(["--".into(), text.into()]);
        input = None;
    } else {
        return Err(
            "no speech on this system: install espeak-ng, or name a voice in \
                    [voice.engines] in config.toml"
                .into(),
        );
    }
    Ok(Clip::Run { argv, input })
}

/// What plays sound here.
fn default_player() -> Result<Vec<String>, String> {
    let players: &[&[&str]] = &[
        &["afplay"],
        &["ffplay", "-nodisp", "-autoexit", "-loglevel", "quiet"],
        &["mpv", "--really-quiet", "--no-video"],
    ];
    players
        .iter()
        .find(|p| found(p[0]))
        .map(|p| p.iter().map(|s| s.to_string()).collect())
        .ok_or_else(|| "nothing here plays sound: install ffmpeg or mpv, or name a `player`".into())
}

/// Says the clip, until it is done or the reader goes elsewhere.
fn play(clip: Clip, heard: &Receiver<Heard>, ours: &mut Vec<Pos>) -> Result<Said, String> {
    let (argv, input, sound) = match clip {
        Clip::Run { argv, input } => (argv, input, None),
        Clip::Sound { path, player } => {
            let mut argv = player;
            argv.push(path.to_string_lossy().into_owned());
            (argv, None, Some(path))
        }
    };
    let result = (|| {
        let mut child = spawn(&argv, input.is_some())?;
        if let Some(text) = input
            && let Some(mut stdin) = child.stdin.take()
        {
            let _ = stdin.write_all(text.as_bytes());
        }
        loop {
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                if status.success() {
                    return Ok(Said::Done);
                }
                let mut err = String::new();
                if let Some(mut e) = child.stderr.take() {
                    let _ = e.read_to_string(&mut err);
                }
                return Err(format!("{} failed: {}", argv[0], err.trim()));
            }
            if let Some(said) = moved(heard, ours) {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(said);
            }
            thread::sleep(Duration::from_millis(30));
        }
    })();
    if let Some(path) = sound {
        let _ = std::fs::remove_file(path);
    }
    result
}

fn spawn(argv: &[String], input: bool) -> Result<Child, String> {
    let (program, args) = argv.split_first().ok_or("no program to run")?;
    Command::new(program)
        .args(args)
        .stdin(if input { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run {program}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engines_are_named_in_config() {
        let file: toml::Table = toml::from_str(
            r#"
            [voice]
            engine = "kyoko"
            code = "read"
            [voice.engines.kyoko]
            type = "command"
            command = ["say", "-v", "Kyoko"]
            [voice.engines.openai]
            type = "openai"
            voice = "coral"
            instructions = "calm"
            "#,
        )
        .unwrap();
        let config: Config = file["voice"].clone().try_into().unwrap();
        assert_eq!(config.engine.as_deref(), Some("kyoko"));
        assert_eq!(config.code, Code::Read);
        let all = engines(&config);
        assert!(all.contains_key("system") && all.contains_key("voiceover"));
        assert!(matches!(&all["openai"], Engine::Openai { voice: Some(v), .. } if v == "coral"));
    }

    #[test]
    fn a_command_takes_the_sentence_on_stdin_or_in_its_arguments() {
        let on_stdin = Engine::Command {
            command: vec!["say".into(), "-v".into(), "Kyoko".into()],
        };
        let Clip::Run { argv, input } = on_stdin.prepare("こんにちは", 0).unwrap() else {
            panic!()
        };
        assert_eq!((argv.len(), input.as_deref()), (3, Some("こんにちは")));
        let placed = Engine::Command {
            command: vec!["tts".into(), "--text={}".into()],
        };
        let Clip::Run { argv, input } = placed.prepare("hi", 0).unwrap() else {
            panic!()
        };
        assert_eq!((argv[1].as_str(), input), ("--text=hi", None));
    }

    #[test]
    fn the_voices_own_turns_are_not_taken_for_moving() {
        let (tx, heard) = mpsc::channel();
        let p = |line| Pos { line, offset: 0 };
        let mut ours = vec![p(10)];
        tx.send(Heard::Moved(p(10))).unwrap();
        assert!(moved(&heard, &mut ours).is_none());
        assert!(ours.is_empty());
        tx.send(Heard::Moved(p(3))).unwrap();
        assert!(matches!(moved(&heard, &mut ours), Some(Said::Moved(at)) if at == p(3)));
    }
}
