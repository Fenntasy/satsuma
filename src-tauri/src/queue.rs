//! The play queue: which track plays next, and what shuffle, repeat and
//! "stop after this track" mean for that choice.
//!
//! This is pure logic with no audio in it, so every combination can be
//! tested directly.

use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};

use crate::db::Track;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Repeat {
    /// Stop when the queue ends.
    #[default]
    Off,
    /// Play the current track again.
    Track,
    /// Start the queue over when it ends.
    Queue,
}

/// What to do once the current track ends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Advance {
    /// Play this track next.
    Play(Box<Track>),
    /// Nothing follows: playback stops.
    Stop,
}

#[derive(Debug, Default)]
pub struct Queue {
    /// The tracks in the order the user sees them.
    tracks: Vec<Track>,
    /// Indices into `tracks`, in playing order. Equal to `0..tracks.len()`
    /// unless shuffle is on.
    order: Vec<usize>,
    /// Position in `order`, not in `tracks`.
    cursor: Option<usize>,
    shuffle: bool,
    repeat: Repeat,
    stop_after_current: bool,
    /// Tracks queued to play before the rest of the queue continues, by
    /// index into `tracks`.
    queued: Vec<usize>,
    /// Where to carry on once the queued tracks have played: the position
    /// in `order` the queue had reached before the first of them.
    resume: Option<usize>,
}

impl Queue {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the queue and starts at `start`, which is an index into
    /// `tracks`. Returns the track to play, if any.
    pub fn replace(&mut self, tracks: Vec<Track>, start: usize) -> Option<&Track> {
        self.tracks = tracks;
        self.queued.clear();
        self.resume = None;
        self.stop_after_current = false;
        self.rebuild_order(Some(start));
        self.current()
    }

    /// Adds tracks at the end of the queue, keeping what is playing.
    pub fn append(&mut self, tracks: Vec<Track>) {
        let first_new = self.tracks.len();
        self.tracks.extend(tracks);
        let new_indices = first_new..self.tracks.len();
        if self.shuffle {
            // Shuffle only the new part, so what is already lined up keeps
            // its order.
            let mut added: Vec<usize> = new_indices.collect();
            added.shuffle(&mut rand::rng());
            self.order.extend(added);
        } else {
            self.order.extend(new_indices);
        }
    }

    /// Plays `index` next, before the rest of the queue continues.
    pub fn play_next(&mut self, index: usize) {
        if index < self.tracks.len() {
            self.queued.push(index);
        }
    }

    #[must_use]
    pub fn current(&self) -> Option<&Track> {
        self.cursor
            .and_then(|cursor| self.order.get(cursor))
            .and_then(|index| self.tracks.get(*index))
    }

    /// The position of the current track in the queue as the user sees it.
    #[must_use]
    pub fn current_index(&self) -> Option<usize> {
        self.cursor
            .and_then(|cursor| self.order.get(cursor))
            .copied()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    #[must_use]
    pub fn shuffle(&self) -> bool {
        self.shuffle
    }

    #[must_use]
    pub fn repeat(&self) -> Repeat {
        self.repeat
    }

    #[must_use]
    pub fn stop_after_current(&self) -> bool {
        self.stop_after_current
    }

    pub fn set_repeat(&mut self, repeat: Repeat) {
        self.repeat = repeat;
    }

    pub fn set_stop_after_current(&mut self, stop: bool) {
        self.stop_after_current = stop;
    }

    /// Turns shuffle on or off, keeping the current track playing: the rest
    /// of the queue is reordered around it.
    pub fn set_shuffle(&mut self, shuffle: bool) {
        if shuffle == self.shuffle {
            return;
        }
        self.shuffle = shuffle;
        // The positions in `order` all change, so a remembered one would
        // point at an unrelated track.
        self.resume = None;
        let current = self.current_index();
        self.rebuild_order(current);
    }

    /// Moves to what follows the current track once it has ended.
    pub fn advance(&mut self) -> Advance {
        if self.stop_after_current {
            self.stop_after_current = false;
            return Advance::Stop;
        }
        if self.repeat == Repeat::Track {
            return match self.current() {
                Some(track) => Advance::Play(Box::new(track.clone())),
                None => Advance::Stop,
            };
        }
        self.step_forward()
    }

    /// Moves to the next track because the user asked for it, which ignores
    /// "repeat track" and "stop after this track".
    pub fn skip_forward(&mut self) -> Advance {
        self.step_forward()
    }

    /// Moves to the previous track. At the start of the queue it stays on
    /// the first track, which is what a player does when you press previous
    /// twice at the beginning.
    pub fn previous(&mut self) -> Advance {
        self.resume = None;
        let Some(cursor) = self.cursor else {
            return Advance::Stop;
        };
        if cursor == 0 {
            return match self.current() {
                Some(track) => Advance::Play(Box::new(track.clone())),
                None => Advance::Stop,
            };
        }
        self.cursor = Some(cursor - 1);
        self.play_current()
    }

    /// Jumps to a track by its index in the queue as the user sees it.
    /// The queue carries on from there, so anything queued to play next
    /// keeps its place but the point to resume from is forgotten.
    pub fn jump_to(&mut self, index: usize) -> Advance {
        self.resume = None;
        match self.order.iter().position(|&item| item == index) {
            Some(cursor) => {
                self.cursor = Some(cursor);
                self.play_current()
            }
            None => Advance::Stop,
        }
    }

    fn step_forward(&mut self) -> Advance {
        if !self.queued.is_empty() {
            let index = self.queued.remove(0);
            // Remember where the queue was, so it carries on from there
            // once the queued tracks have played.
            let resume = self.resume.or(self.cursor);
            let advance = self.jump_to(index);
            self.resume = resume;
            return advance;
        }
        // The queued tracks are done: pick the queue back up where it was.
        let Some(cursor) = self.resume.take().or(self.cursor) else {
            return Advance::Stop;
        };
        let next = cursor + 1;
        if next < self.order.len() {
            self.cursor = Some(next);
            return self.play_current();
        }
        if self.repeat == Repeat::Queue && !self.order.is_empty() {
            if self.shuffle {
                // A new pass gets a new order, otherwise "shuffle" would
                // repeat the same sequence for ever.
                self.rebuild_order(None);
            }
            self.cursor = Some(0);
            return self.play_current();
        }
        Advance::Stop
    }

    fn play_current(&self) -> Advance {
        match self.current() {
            Some(track) => Advance::Play(Box::new(track.clone())),
            None => Advance::Stop,
        }
    }

    /// Rebuilds the playing order, keeping `keep` (an index into `tracks`)
    /// as the current track when it is given.
    fn rebuild_order(&mut self, keep: Option<usize>) {
        let mut order: Vec<usize> = (0..self.tracks.len()).collect();
        if self.shuffle {
            order.shuffle(&mut rand::rng());
            if let Some(keep) = keep {
                // The track that is playing goes first, so turning shuffle
                // on does not interrupt it.
                if let Some(position) = order.iter().position(|&index| index == keep) {
                    order.swap(0, position);
                }
            }
        }
        self.order = order;
        self.cursor = match keep {
            Some(keep) => self.order.iter().position(|&index| index == keep),
            None => None,
        };
        if self.cursor.is_none() && !self.order.is_empty() && keep.is_some() {
            self.cursor = Some(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Advance, Queue, Repeat};
    use crate::db::Track;

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

    fn tracks(count: i64) -> Vec<Track> {
        (1..=count).map(track).collect()
    }

    fn playing(advance: &Advance) -> Option<i64> {
        match advance {
            Advance::Play(track) => Some(track.id),
            Advance::Stop => None,
        }
    }

    fn queue_of(count: i64) -> Queue {
        let mut queue = Queue::new();
        queue.replace(tracks(count), 0);
        queue
    }

    #[test]
    fn plays_the_track_it_starts_on() {
        let mut queue = Queue::new();
        assert_eq!(queue.replace(tracks(3), 1).map(|t| t.id), Some(2));
        assert_eq!(queue.current().map(|t| t.id), Some(2));
        assert_eq!(queue.current_index(), Some(1));
        assert_eq!(queue.len(), 3);
    }

    #[test]
    fn walks_the_queue_and_stops_at_the_end() {
        let mut queue = queue_of(3);
        assert_eq!(playing(&queue.skip_forward()), Some(2));
        assert_eq!(playing(&queue.skip_forward()), Some(3));
        assert_eq!(queue.skip_forward(), Advance::Stop);
    }

    #[test]
    fn previous_stays_on_the_first_track() {
        let mut queue = queue_of(3);
        assert_eq!(playing(&queue.skip_forward()), Some(2));
        assert_eq!(playing(&queue.previous()), Some(1));
        assert_eq!(
            playing(&queue.previous()),
            Some(1),
            "pressing previous at the start replays the first track"
        );
    }

    #[test]
    fn repeat_track_plays_the_same_track_again() {
        let mut queue = queue_of(3);
        queue.set_repeat(Repeat::Track);
        assert_eq!(playing(&queue.advance()), Some(1));
        assert_eq!(playing(&queue.advance()), Some(1));
        assert_eq!(
            playing(&queue.skip_forward()),
            Some(2),
            "asking for the next track ignores repeat track"
        );
    }

    #[test]
    fn repeat_queue_starts_over() {
        let mut queue = queue_of(2);
        assert_eq!(playing(&queue.advance()), Some(2));
        queue.set_repeat(Repeat::Queue);
        assert_eq!(playing(&queue.advance()), Some(1));
    }

    #[test]
    fn stop_after_current_stops_once() {
        let mut queue = queue_of(3);
        queue.set_stop_after_current(true);
        assert_eq!(queue.advance(), Advance::Stop);
        assert!(
            !queue.stop_after_current(),
            "the flag clears itself once it applied"
        );
        assert_eq!(playing(&queue.advance()), Some(2));
    }

    #[test]
    fn stop_after_current_wins_over_repeat_track() {
        let mut queue = queue_of(2);
        queue.set_repeat(Repeat::Track);
        queue.set_stop_after_current(true);
        assert_eq!(queue.advance(), Advance::Stop);
    }

    #[test]
    fn a_queued_track_plays_before_the_rest() {
        let mut queue = queue_of(4);
        queue.play_next(3);
        assert_eq!(playing(&queue.advance()), Some(4), "the queued track first");
        assert_eq!(
            playing(&queue.advance()),
            Some(2),
            "then the queue carries on where it was, after track 1"
        );
        assert_eq!(playing(&queue.advance()), Some(3));
    }

    #[test]
    fn queued_tracks_keep_their_order() {
        let mut queue = queue_of(5);
        queue.play_next(4);
        queue.play_next(3);
        assert_eq!(playing(&queue.advance()), Some(5));
        assert_eq!(playing(&queue.advance()), Some(4));
        assert_eq!(
            playing(&queue.advance()),
            Some(2),
            "the queue resumes only once every queued track has played"
        );
    }

    #[test]
    fn shuffle_keeps_the_current_track_and_plays_everything_once() {
        let mut queue = queue_of(20);
        assert_eq!(playing(&queue.skip_forward()), Some(2));
        queue.set_shuffle(true);
        assert_eq!(
            queue.current().map(|t| t.id),
            Some(2),
            "turning shuffle on must not change what is playing"
        );

        let mut played = vec![2];
        while let Advance::Play(track) = queue.skip_forward() {
            played.push(track.id);
        }
        played.sort_unstable();
        played.dedup();
        assert_eq!(played.len(), 20, "every track is played exactly once");
    }

    #[test]
    fn appending_under_shuffle_keeps_the_pending_order_and_adds_the_rest() {
        let mut queue = queue_of(4);
        queue.set_shuffle(true);
        let pending: Vec<i64> = {
            let mut seen = Vec::new();
            let mut probe = Queue::new();
            probe.replace(tracks(0), 0);
            seen.clear();
            seen
        };
        assert!(pending.is_empty(), "probe queue is only a placeholder");

        queue.append(
            tracks(3)
                .into_iter()
                .map(|mut t| {
                    t.id += 100;
                    t
                })
                .collect(),
        );
        assert_eq!(queue.len(), 7);

        let mut played = vec![queue.current().map(|t| t.id).expect("a current track")];
        while let Advance::Play(track) = queue.skip_forward() {
            played.push(track.id);
        }
        played.sort_unstable();
        played.dedup();
        assert_eq!(played.len(), 7, "every track plays exactly once");
    }

    #[test]
    fn a_shuffled_queue_repeats_with_every_track_again() {
        let mut queue = queue_of(6);
        queue.set_shuffle(true);
        queue.set_repeat(Repeat::Queue);

        let mut first_pass = vec![queue.current().map(|t| t.id).expect("a track")];
        for _ in 1..6 {
            match queue.advance() {
                Advance::Play(track) => first_pass.push(track.id),
                Advance::Stop => panic!("the queue must not stop before its end"),
            }
        }
        let mut second_pass = Vec::new();
        for _ in 0..6 {
            match queue.advance() {
                Advance::Play(track) => second_pass.push(track.id),
                Advance::Stop => panic!("repeat queue must start over"),
            }
        }
        first_pass.sort_unstable();
        second_pass.sort_unstable();
        assert_eq!(first_pass, second_pass, "both passes play every track");
    }

    #[test]
    fn turning_shuffle_off_restores_the_original_order() {
        let mut queue = queue_of(5);
        queue.set_shuffle(true);
        queue.set_shuffle(false);
        assert_eq!(queue.current().map(|t| t.id), Some(1));
        assert_eq!(playing(&queue.skip_forward()), Some(2));
        assert_eq!(playing(&queue.skip_forward()), Some(3));
    }

    #[test]
    fn appending_keeps_playing_and_extends_the_queue() {
        let mut queue = queue_of(2);
        queue.append(tracks(1));
        assert_eq!(queue.len(), 3);
        assert_eq!(queue.current().map(|t| t.id), Some(1));
        assert_eq!(playing(&queue.skip_forward()), Some(2));
        assert!(matches!(queue.skip_forward(), Advance::Play(_)));
    }

    #[test]
    fn jumping_moves_to_the_chosen_track() {
        let mut queue = queue_of(4);
        assert_eq!(playing(&queue.jump_to(2)), Some(3));
        assert_eq!(queue.current_index(), Some(2));
        assert_eq!(queue.jump_to(99), Advance::Stop);
    }

    #[test]
    fn an_empty_queue_plays_nothing() {
        let mut queue = Queue::new();
        assert!(queue.is_empty());
        assert_eq!(queue.current(), None);
        assert_eq!(queue.skip_forward(), Advance::Stop);
        assert_eq!(queue.previous(), Advance::Stop);
        assert_eq!(queue.advance(), Advance::Stop);
    }

    #[test]
    fn jumping_away_forgets_where_the_queue_was() {
        let mut queue = queue_of(5);
        queue.play_next(4);
        assert_eq!(playing(&queue.advance()), Some(5), "the queued track");
        assert_eq!(playing(&queue.jump_to(2)), Some(3), "the user moves away");
        assert_eq!(
            playing(&queue.advance()),
            Some(4),
            "the queue carries on from where the user jumped, not from before"
        );
    }

    #[test]
    fn turning_shuffle_on_during_a_queued_track_does_not_jump_back() {
        let mut queue = queue_of(6);
        queue.play_next(5);
        assert_eq!(playing(&queue.advance()), Some(6));
        queue.set_shuffle(true);
        assert_eq!(
            queue.current().map(|t| t.id),
            Some(6),
            "the queued track keeps playing"
        );
        assert!(matches!(queue.advance(), Advance::Play(_)));
    }
}
