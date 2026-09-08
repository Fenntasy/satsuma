//! Playback: a thread that owns the audio output and answers commands.
//!
//! The audio device and the decoder are not shareable, so they stay on one
//! thread and the rest of the application talks to it through a channel.
//! The queue logic it drives lives in [`crate::queue`].

use std::fs::File;
use std::path::Path;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Weak};
use std::time::Duration;

use rodio::source::EmptyCallback;
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Source};
use serde::{Deserialize, Serialize};

use crate::db::Track;
use crate::queue::{Advance, Queue, Repeat};

/// How often the thread reports where it is when playing.
const TICK: Duration = Duration::from_millis(200);

/// How many unplayable tracks in a row are skipped before giving up, so a
/// queue whose files all moved stops instead of racing to its end.
const MAX_SKIPS: usize = 20;

/// A track that ends sooner than this never really played: a tag-only or
/// truncated file. Those count against [`MAX_SKIPS`] too, otherwise a queue
/// of them would spin at channel speed.
const MIN_PLAYTIME: Duration = Duration::from_millis(50);

/// What the frontend can ask the player to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Replace the queue and start playing at `start`.
    Play {
        tracks: Vec<Track>,
        start: usize,
    },
    /// Add tracks at the end of the queue.
    Enqueue(Vec<Track>),
    /// Play this queue position next.
    PlayNext(usize),
    /// Jump to this queue position now.
    JumpTo(usize),
    PlayPause,
    Stop,
    Next,
    Previous,
    Seek(Duration),
    SetVolume(f32),
    SetShuffle(bool),
    SetRepeat(Repeat),
    SetStopAfterCurrent(bool),
    /// Send the current state back, without changing anything.
    ReportState,
    /// The track started in `generation` played to its end. Sent by the
    /// audio thread itself, not by the frontend.
    TrackFinished(u64),
    /// End the thread.
    Shutdown,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    #[default]
    Stopped,
    Playing,
    Paused,
}

/// What the frontend shows.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub status: Status,
    pub track: Option<Track>,
    pub position_ms: u64,
    pub volume: f32,
    pub shuffle: bool,
    pub repeat: Repeat,
    pub stop_after_current: bool,
    pub queue_length: usize,
    /// How many seeks the player has carried out. The frontend watches this
    /// to know its own seek was applied, whether it landed or failed.
    pub seeks_applied: u64,
    /// Set when the last command could not be carried out, e.g. a file that
    /// disappeared since it was scanned.
    pub error: Option<String>,
}

/// Runs the player until [`Command::Shutdown`], or until every [`Handle`]
/// has been dropped.
///
/// The player keeps a sender of its own to post end-of-track messages, so
/// the channel never closes by itself; `alive` is what says whether anyone
/// outside can still send commands.
///
/// `on_state` is called whenever something changes and while playing, at
/// most every [`TICK`].
pub fn run(
    commands: &Receiver<Command>,
    notify: &Sender<Command>,
    alive: &Weak<()>,
    mut on_state: impl FnMut(State),
) {
    let mut player = match Player::new(notify.clone()) {
        Ok(player) => player,
        Err(err) => {
            log::error!("no audio output ({err}); playback is disabled");
            let state = State {
                error: Some(err),
                // Not the `Default` zero: the slider would read 0% and
                // suggest a second problem that does not exist.
                volume: 1.0,
                ..State::default()
            };
            on_state(state.clone());
            drain(commands, alive, &state, on_state);
            return;
        }
    };
    on_state(player.state());
    loop {
        match commands.recv_timeout(TICK) {
            Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => return,
            Ok(command) => {
                player.handle(command);
                on_state(player.state());
            }
            Err(RecvTimeoutError::Timeout) => {
                if alive.upgrade().is_none() {
                    return;
                }
                // Report where we are, so the seek bar moves.
                if player.status == Status::Playing {
                    on_state(player.state());
                }
            }
        }
    }
}

/// Answers commands without playing anything, which is what happens when
/// there is no audio device. Every command still gets the failure back, so
/// a frontend that starts after the first report still learns about it.
fn drain(
    commands: &Receiver<Command>,
    alive: &Weak<()>,
    state: &State,
    mut on_state: impl FnMut(State),
) {
    loop {
        match commands.recv_timeout(TICK) {
            Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => return,
            Ok(_) => on_state(state.clone()),
            Err(RecvTimeoutError::Timeout) => {
                if alive.upgrade().is_none() {
                    return;
                }
            }
        }
    }
}

struct Player {
    /// Dropping this stops the audio output, so it is kept alive even
    /// though nothing reads it after the mixer is taken.
    _device: MixerDeviceSink,
    sink: rodio::Player,
    queue: Queue,
    status: Status,
    volume: f32,
    /// The duration the decoder reported, which is more reliable than the
    /// one in the tags for files without a Xing header.
    duration: Option<Duration>,
    error: Option<String>,
    /// Bumped every time a track starts, so the end-of-track message of a
    /// track the user skipped away from is recognised and ignored.
    generation: u64,
    seeks_applied: u64,
    /// When the track that is playing started, and how many tracks in a row
    /// have ended without really playing.
    started_at: Option<std::time::Instant>,
    empty_tracks: usize,
    notify: Sender<Command>,
}

impl Player {
    fn new(notify: Sender<Command>) -> Result<Self, String> {
        let mut device = DeviceSinkBuilder::open_default_sink().map_err(|err| err.to_string())?;
        // The default prints to stderr when the app closes.
        device.log_on_drop(false);
        let sink = rodio::Player::connect_new(device.mixer());
        Ok(Player {
            _device: device,
            sink,
            queue: Queue::new(),
            status: Status::Stopped,
            volume: 1.0,
            duration: None,
            error: None,
            generation: 0,
            seeks_applied: 0,
            started_at: None,
            empty_tracks: 0,
            notify,
        })
    }

    fn handle(&mut self, command: Command) {
        // Asking where we are must not erase why the last attempt failed.
        if !matches!(command, Command::ReportState | Command::TrackFinished(_)) {
            self.error = None;
        }
        // Anything the user asks for is a fresh start for the "nothing
        // plays" budget.
        if !matches!(command, Command::TrackFinished(_) | Command::ReportState) {
            self.empty_tracks = 0;
        }
        match command {
            Command::Play { tracks, start } => {
                let track = self.queue.replace(tracks, start).cloned();
                self.start(track.as_ref());
            }
            Command::Enqueue(tracks) => {
                let was_empty = self.queue.is_empty();
                self.queue.append(tracks);
                if was_empty {
                    let track = self.queue.jump_to(0);
                    self.play_advance(track);
                }
            }
            Command::PlayNext(index) => self.queue.play_next(index),
            Command::JumpTo(index) => {
                let advance = self.queue.jump_to(index);
                self.play_advance(advance);
            }
            Command::PlayPause => self.play_pause(),
            Command::Stop => self.stop(),
            Command::Next => {
                let advance = self.queue.skip_forward();
                self.play_advance(advance);
            }
            Command::Previous => {
                // Restart the track when it is well underway, like every
                // other player: pressing previous means "from the top".
                if self.sink.get_pos() > Duration::from_secs(3) {
                    let current = self.queue.current().cloned();
                    self.start(current.as_ref());
                } else {
                    let advance = self.queue.previous();
                    self.play_advance(advance);
                }
            }
            Command::Seek(position) => self.seek(position),
            Command::SetVolume(volume) => {
                self.volume = volume.clamp(0.0, 1.0);
                self.sink.set_volume(self.volume);
            }
            Command::SetShuffle(shuffle) => self.queue.set_shuffle(shuffle),
            Command::SetRepeat(repeat) => self.queue.set_repeat(repeat),
            Command::SetStopAfterCurrent(stop) => self.queue.set_stop_after_current(stop),
            Command::TrackFinished(generation) => {
                // A track the user skipped away from also reports its end.
                if generation == self.generation && self.status == Status::Playing {
                    let played_nothing = self
                        .started_at
                        .is_some_and(|started| started.elapsed() < MIN_PLAYTIME);
                    if played_nothing {
                        self.empty_tracks += 1;
                        if self.empty_tracks >= MAX_SKIPS {
                            log::warn!("giving up after {MAX_SKIPS} tracks that played nothing");
                            self.error = Some("these files hold no audio to play".to_owned());
                            self.stop();
                            return;
                        }
                    } else {
                        self.empty_tracks = 0;
                    }
                    let advance = self.queue.advance();
                    self.play_advance(advance);
                }
            }
            Command::ReportState | Command::Shutdown => {}
        }
    }

    fn play_advance(&mut self, advance: Advance) {
        match advance {
            Advance::Play(track) => self.start(Some(&track)),
            Advance::Stop => self.stop(),
        }
    }

    /// Starts `track`, moving on to the next one when it cannot be played:
    /// one file that moved since the scan must not end the session.
    fn start(&mut self, track: Option<&Track>) {
        let mut next = track.cloned();
        for _ in 0..MAX_SKIPS {
            let Some(track) = next else {
                self.stop();
                return;
            };
            match self.try_start(&track) {
                Ok(()) => return,
                Err(err) => {
                    log::warn!("skipping {}: {err}", track.path);
                    self.error = Some(err);
                    next = match self.queue.skip_forward() {
                        Advance::Play(track) => Some(*track),
                        Advance::Stop => None,
                    };
                }
            }
        }
        log::warn!("giving up after {MAX_SKIPS} tracks that could not be played");
        self.stop();
    }

    fn try_start(&mut self, track: &Track) -> Result<(), String> {
        self.sink.clear();
        self.generation += 1;
        let source = decode(Path::new(&track.path))?;
        self.duration = source.total_duration();
        self.sink.append(source);
        // A zero-length source right after the track tells us it ended,
        // which is immediate where polling would leave a gap.
        let notify = self.notify.clone();
        let generation = self.generation;
        self.sink.append(EmptyCallback::new(Box::new(move || {
            let _ = notify.send(Command::TrackFinished(generation));
        })));
        self.sink.play();
        self.status = Status::Playing;
        self.started_at = Some(std::time::Instant::now());
        Ok(())
    }

    fn play_pause(&mut self) {
        match self.status {
            Status::Playing => {
                self.sink.pause();
                self.status = Status::Paused;
            }
            Status::Paused => {
                self.sink.play();
                self.status = Status::Playing;
            }
            Status::Stopped => {
                let track = self.queue.current().cloned().or_else(|| {
                    // Nothing was playing: start the queue from its first
                    // track.
                    match self.queue.jump_to(0) {
                        Advance::Play(track) => Some(*track),
                        Advance::Stop => None,
                    }
                });
                self.start(track.as_ref());
            }
        }
    }

    fn stop(&mut self) {
        self.sink.clear();
        self.status = Status::Stopped;
        self.duration = None;
        self.started_at = None;
    }

    fn seek(&mut self, position: Duration) {
        // Counted even when it cannot be done, so the frontend stops
        // showing the position the user asked for.
        self.seeks_applied += 1;
        // Seeking an empty queue reports success without doing anything.
        if self.status == Status::Stopped || self.sink.empty() {
            return;
        }
        if let Err(err) = self.sink.try_seek(position) {
            log::warn!("cannot seek: {err}");
            self.error = Some(format!("cannot seek in this file: {err}"));
        }
    }

    fn state(&self) -> State {
        let track = self.queue.current().cloned().map(|mut track| {
            // The decoder knows better than the tags for files whose header
            // does not carry a length.
            if let Some(duration) = self.duration {
                let millis = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
                if millis > 0 {
                    track.duration_ms = millis;
                }
            }
            track
        });
        State {
            status: self.status,
            position_ms: u64::try_from(self.sink.get_pos().as_millis()).unwrap_or(u64::MAX),
            track: if self.status == Status::Stopped {
                None
            } else {
                track
            },
            volume: self.volume,
            shuffle: self.queue.shuffle(),
            repeat: self.queue.repeat(),
            stop_after_current: self.queue.stop_after_current(),
            queue_length: self.queue.len(),
            seeks_applied: self.seeks_applied,
            error: self.error.clone(),
        }
    }
}

fn decode(path: &Path) -> Result<Decoder<std::io::BufReader<File>>, String> {
    let file = File::open(path).map_err(|err| err.to_string())?;
    Decoder::try_from(file).map_err(|err| err.to_string())
}

/// The handle the rest of the application keeps: sending on it wakes the
/// player thread. When the last handle is dropped the thread ends, which
/// the channel alone cannot tell because the player holds a sender too.
#[derive(Clone, Debug)]
pub struct Handle {
    commands: Sender<Command>,
    alive: Arc<()>,
}

impl Handle {
    #[must_use]
    pub fn new(commands: Sender<Command>) -> Self {
        Handle {
            commands,
            alive: Arc::new(()),
        }
    }

    /// The token the player thread watches to know this handle still exists.
    #[must_use]
    pub fn liveness(&self) -> Weak<()> {
        Arc::downgrade(&self.alive)
    }

    /// Sends a command, reporting whether the player thread is still there.
    ///
    /// # Errors
    ///
    /// Returns a message when the player thread has stopped.
    pub fn send(&self, command: Command) -> Result<(), String> {
        self.commands
            .send(command)
            .map_err(|_| "the player stopped".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::{run, Command, State, Status};
    use crate::db::Track;
    use crate::queue::Repeat;

    fn track(id: i64) -> Track {
        Track {
            id,
            path: format!("/music/{id}.mp3"),
            title: Some(format!("Track {id}")),
            artist: None,
            album: None,
            duration_ms: 1000,
        }
    }

    /// Runs the player until it stops, collecting every state it reported.
    ///
    /// There is no audio device in CI, so this exercises the "playback is
    /// disabled" path: the thread must answer, report the failure once and
    /// then shut down cleanly instead of hanging or spinning.
    fn drive(commands: Vec<Command>) -> Vec<State> {
        let (sender, receiver) = mpsc::channel();
        let notify = sender.clone();
        for command in commands {
            sender.send(command).expect("send");
        }
        sender.send(Command::Shutdown).expect("send");
        let (states_tx, states_rx) = mpsc::channel();
        let alive = std::sync::Arc::new(());
        let watch = std::sync::Arc::downgrade(&alive);
        let worker = std::thread::spawn(move || {
            run(&receiver, &notify, &watch, |state| {
                let _ = states_tx.send(state);
            });
        });
        worker.join().expect("the player thread must not panic");
        states_rx.into_iter().collect()
    }

    #[test]
    fn reports_its_state_and_shuts_down() {
        let states = drive(vec![Command::ReportState]);
        assert!(!states.is_empty(), "the player must report at least once");
    }

    #[test]
    fn commands_never_panic_without_an_audio_device() {
        // Whether CI has a sound card or not, none of these may bring the
        // thread down.
        let states = drive(vec![
            Command::Play {
                tracks: vec![track(1), track(2)],
                start: 0,
            },
            Command::SetVolume(0.5),
            Command::SetShuffle(true),
            Command::SetRepeat(Repeat::Queue),
            Command::SetStopAfterCurrent(true),
            Command::Seek(Duration::from_secs(5)),
            Command::Next,
            Command::Previous,
            Command::PlayPause,
            Command::Stop,
            Command::Enqueue(vec![track(3)]),
            Command::PlayNext(0),
            Command::JumpTo(0),
            Command::TrackFinished(0),
            Command::ReportState,
        ]);
        assert!(!states.is_empty());
    }

    #[test]
    fn a_missing_file_is_reported_without_stopping_the_player() {
        let states = drive(vec![
            Command::Play {
                tracks: vec![track(1)],
                start: 0,
            },
            Command::ReportState,
        ]);
        let last = states.last().expect("a state");
        assert_eq!(last.status, Status::Stopped, "nothing can play");
        // Either there is no audio device, or the file does not exist:
        // both must surface rather than being swallowed.
        assert!(
            last.error.is_some() || states.iter().any(|state| state.error.is_some()),
            "the failure must be reported"
        );
    }

    /// Drives a real player thread, so the decoder and the audio device are
    /// exercised rather than mocked. Returns `None` when the machine has no
    /// audio output, which is the case on CI.
    fn with_audio(
        tracks: Vec<Track>,
        act: impl FnOnce(&mpsc::Sender<Command>, &mpsc::Receiver<State>),
    ) -> Option<Vec<State>> {
        with_audio_setup(Vec::new(), tracks, act)
    }

    /// As [`with_audio`], with commands sent before playback starts.
    fn with_audio_setup(
        setup: Vec<Command>,
        tracks: Vec<Track>,
        act: impl FnOnce(&mpsc::Sender<Command>, &mpsc::Receiver<State>),
    ) -> Option<Vec<State>> {
        let (sender, receiver) = mpsc::channel();
        let notify = sender.clone();
        let handle = super::Handle::new(sender.clone());
        let watch = handle.liveness();
        let (states_tx, states_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            run(&receiver, &notify, &watch, |state| {
                let _ = states_tx.send(state);
            });
        });

        let first = states_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the player must report its state at once");
        if first.error.is_some() {
            assert!(
                std::env::var("SATSUMA_REQUIRE_AUDIO").is_err(),
                "no audio output ({:?}), but SATSUMA_REQUIRE_AUDIO is set: \
                 the audio tests must not silently skip here",
                first.error
            );
            drop(handle);
            worker.join().expect("the player thread must stop");
            return None;
        }

        for command in setup {
            sender.send(command).expect("send");
        }
        sender
            .send(Command::Play { tracks, start: 0 })
            .expect("send");
        act(&sender, &states_rx);
        let seen: Vec<State> = states_rx.try_iter().collect();
        drop(handle);
        worker.join().expect("the player thread must stop");
        Some(seen)
    }

    /// Waits for a state matching `wanted`, collecting everything seen.
    fn wait_for(
        states: &mpsc::Receiver<State>,
        timeout: Duration,
        wanted: impl Fn(&State) -> bool,
    ) -> Option<State> {
        let deadline = std::time::Instant::now() + timeout;
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            match states.recv_timeout(left) {
                Ok(state) if wanted(&state) => return Some(state),
                Ok(_) => {}
                Err(_) => return None,
            }
        }
        None
    }

    fn fixture(id: i64) -> Track {
        Track {
            id,
            path: concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/silence.mp3").to_owned(),
            title: Some(format!("Fixture {id}")),
            artist: None,
            album: None,
            duration_ms: 1000,
        }
    }

    #[test]
    fn plays_a_real_file_and_moves_through_the_queue() {
        let played = with_audio(vec![fixture(1), fixture(2)], |sender, states| {
            let playing = wait_for(states, Duration::from_secs(5), |state| {
                state.status == Status::Playing && state.track.is_some()
            })
            .expect("the first track must start playing");
            assert_eq!(playing.track.as_ref().map(|track| track.id), Some(1));

            // The one-second fixture ends on its own and the queue moves on.
            let second = wait_for(states, Duration::from_secs(10), |state| {
                state.track.as_ref().is_some_and(|track| track.id == 2)
            })
            .expect("the queue must advance when a track ends");
            assert_eq!(second.status, Status::Playing);

            sender.send(Command::PlayPause).expect("send");
            let paused = wait_for(states, Duration::from_secs(5), |state| {
                state.status == Status::Paused
            })
            .expect("pausing must take effect");
            assert!(paused.track.is_some(), "a paused player keeps its track");

            sender.send(Command::Stop).expect("send");
            let stopped = wait_for(states, Duration::from_secs(5), |state| {
                state.status == Status::Stopped
            })
            .expect("stopping must take effect");
            assert_eq!(stopped.track, None, "nothing plays once stopped");
        });
        if played.is_none() {
            eprintln!("skipped: this machine has no audio output");
        }
    }

    #[test]
    fn an_unplayable_track_is_skipped_rather_than_ending_the_session() {
        let missing = Track {
            id: 9,
            path: "/definitely/missing.mp3".to_owned(),
            title: Some("Missing".to_owned()),
            artist: None,
            album: None,
            duration_ms: 1000,
        };
        let played = with_audio(vec![missing, fixture(2)], |_sender, states| {
            let playing = wait_for(states, Duration::from_secs(5), |state| {
                state.status == Status::Playing
            })
            .expect("the next playable track must start");
            assert_eq!(
                playing.track.as_ref().map(|track| track.id),
                Some(2),
                "the file that cannot be read is skipped"
            );
            assert!(
                playing.error.is_some(),
                "the skipped file is still reported"
            );
        });
        if played.is_none() {
            eprintln!("skipped: this machine has no audio output");
        }
    }

    #[test]
    fn a_queue_of_unplayable_tracks_stops_instead_of_spinning() {
        let missing = |id: i64| Track {
            id,
            path: format!("/definitely/missing{id}.mp3"),
            title: None,
            artist: None,
            album: None,
            duration_ms: 1000,
        };
        let played = with_audio_setup(
            vec![Command::SetRepeat(Repeat::Queue)],
            (1..=5).map(missing).collect(),
            |sender, states| {
                sender.send(Command::ReportState).expect("send");
                let stopped = wait_for(states, Duration::from_secs(5), |state| {
                    state.status == Status::Stopped && state.error.is_some()
                });
                assert!(
                    stopped.is_some(),
                    "a queue that repeats and can play nothing must still stop"
                );
            },
        );
        if played.is_none() {
            eprintln!("skipped: this machine has no audio output");
        }
    }

    #[test]
    fn seeking_is_counted_even_when_it_cannot_be_done() {
        let played = with_audio(vec![fixture(1)], |sender, states| {
            let before = wait_for(states, Duration::from_secs(5), |state| {
                state.status == Status::Playing
            })
            .expect("the track must start")
            .seeks_applied;
            sender
                .send(Command::Seek(Duration::from_millis(200)))
                .expect("send");
            let after = wait_for(states, Duration::from_secs(5), |state| {
                state.seeks_applied > before
            });
            assert!(
                after.is_some(),
                "the frontend needs to know the seek was handled"
            );
        });
        if played.is_none() {
            eprintln!("skipped: this machine has no audio output");
        }
    }

    #[test]
    fn position_advances_while_playing() {
        let played = with_audio(vec![fixture(1)], |_sender, states| {
            let moved = wait_for(states, Duration::from_secs(5), |state| {
                state.status == Status::Playing && state.position_ms > 100
            });
            assert!(
                moved.is_some(),
                "the reported position must move while a track plays"
            );
        });
        if played.is_none() {
            eprintln!("skipped: this machine has no audio output");
        }
    }

    #[test]
    fn the_thread_stops_when_the_handle_is_dropped() {
        let (sender, receiver) = mpsc::channel::<Command>();
        let notify = sender.clone();
        let handle = super::Handle::new(sender);
        let watch = handle.liveness();
        let worker = std::thread::spawn(move || {
            run(&receiver, &notify, &watch, |_| {});
        });
        // The thread keeps a sender of its own for end-of-track messages,
        // so closing the channel is not what ends it: dropping the last
        // handle is.
        drop(handle);
        worker.join().expect("the player thread must stop");
    }
}
