//! A call being written down as it happens: who came and went, who shared a
//! screen, who recorded, and what was said if someone's Mac transcribed it.
//! When the call ends it becomes a [`CallRecord`] and its [`Moment`]s, for
//! the Calls tab and the room's history.

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::store::{CallRecord, Moment, MomentKind};

/// Whoever is reported this soon after you got in was there already: they
/// count as people in the call, not as having joined.
const ALREADY_THERE: Duration = Duration::from_millis(500);
/// The most transcript lines kept for one call (a long meeting's worth).
const MAX_LINES: usize = 2_000;

pub(crate) struct Log {
    /// When you got in: on the wall clock (unix ms), and for measuring.
    began: u64,
    since: Instant,
    /// Your identity in the call, and what you're called.
    you: String,
    your_name: String,
    names: HashMap<String, String>,
    /// Shared screens showing now: track → whose.
    screens: HashMap<String, String>,
    people: Vec<String>,
    moments: Vec<Moment>,
    lines: usize,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

impl Log {
    /// You're in, as `you` (an identity), called `your_name`.
    pub(crate) fn begin(you: String, your_name: String) -> Self {
        Self {
            began: now(),
            since: Instant::now(),
            you,
            your_name,
            names: HashMap::new(),
            screens: HashMap::new(),
            people: Vec::new(),
            moments: Vec::new(),
            lines: 0,
        }
    }

    fn name(&self, identity: &str) -> String {
        if identity == self.you {
            return self.your_name.clone();
        }
        self.names
            .get(identity)
            .cloned()
            .unwrap_or_else(|| "Someone".into())
    }

    fn note(&mut self, kind: MomentKind, identity: &str) {
        self.moments.push(Moment {
            at: now() / 1000,
            kind,
            who: self.name(identity),
            mine: identity == self.you,
            text: String::new(),
            key: String::new(),
        });
    }

    pub(crate) fn joined(&mut self, identity: &str, name: &str) {
        let name = if name.is_empty() { "Guest" } else { name };
        self.names.insert(identity.to_owned(), name.to_owned());
        if !self.people.iter().any(|p| p == name) {
            self.people.push(name.to_owned());
        }
        if self.since.elapsed() >= ALREADY_THERE {
            self.note(MomentKind::Joined, identity);
        }
    }

    pub(crate) fn left(&mut self, identity: &str) {
        self.note(MomentKind::Left, identity);
    }

    /// A video appeared; only shared screens are worth a line.
    pub(crate) fn screen_started(&mut self, track: &str, identity: &str) {
        self.screens.insert(track.to_owned(), identity.to_owned());
        self.note(MomentKind::ShareStarted, identity);
    }

    /// A video went: if it was a shared screen, that share is over.
    pub(crate) fn track_removed(&mut self, track: &str) {
        if let Some(identity) = self.screens.remove(track) {
            self.note(MomentKind::ShareStopped, &identity);
        }
    }

    pub(crate) fn recording(&mut self, identity: &str, on: bool) {
        let kind = if on {
            MomentKind::RecordingStarted
        } else {
            MomentKind::RecordingStopped
        };
        self.note(kind, identity);
    }

    pub(crate) fn transcribing(&mut self, identity: &str, on: bool) {
        let kind = if on {
            MomentKind::TranscriptStarted
        } else {
            MomentKind::TranscriptStopped
        };
        self.note(kind, identity);
    }

    /// A finished line of the transcript. A later one with the same `key`
    /// replaces it; one with no text takes it back.
    pub(crate) fn said(&mut self, key: &str, speaker: &str, at_ms: u64, text: &str) {
        let same = self.moments.iter().rposition(|m| m.key == key);
        if text.is_empty() {
            if let Some(at) = same {
                self.moments.remove(at);
                self.lines -= 1;
            }
            return;
        }
        match same {
            Some(at) => self.moments[at].text = text.to_owned(),
            None if self.lines < MAX_LINES => {
                self.lines += 1;
                self.moments.push(Moment {
                    at: at_ms / 1000,
                    kind: MomentKind::Said,
                    who: self.name(speaker),
                    mine: speaker == self.you,
                    text: text.to_owned(),
                    key: key.to_owned(),
                });
            }
            None => {}
        }
    }

    /// The call is over: its record and everything that happened, in order.
    pub(crate) fn finish(mut self, room: &str, name: &str) -> (CallRecord, Vec<Moment>) {
        // A line is dated when it was said, which is before it arrived.
        self.moments.sort_by_key(|m| m.at);
        let record = CallRecord {
            id: self.began,
            room: room.to_owned(),
            name: if name.is_empty() { "Call" } else { name }.to_owned(),
            started: self.began / 1000,
            seconds: self.since.elapsed().as_secs(),
            people: self.people,
        };
        (record, self.moments)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(moments: &[Moment]) -> Vec<(MomentKind, &str)> {
        moments.iter().map(|m| (m.kind, m.who.as_str())).collect()
    }

    #[test]
    fn a_call_is_written_down_in_order() {
        let mut log = Log::begin("me-1".into(), "Nikita".into());
        // Anna was there when you got in; Max comes later.
        log.joined("anna-1", "Anna");
        log.since -= Duration::from_secs(5);
        log.joined("max-1", "Max");
        log.screen_started("TR_screen", "anna-1");
        log.recording("anna-1", true);
        log.transcribing("anna-1", true);
        log.track_removed("TR_camera");
        log.track_removed("TR_screen");
        log.left("max-1");
        let (record, moments) = log.finish("room", "Team");

        assert_eq!(record.people, ["Anna", "Max"]);
        assert_eq!(record.name, "Team");
        assert!(record.seconds >= 5);
        assert_eq!(
            kinds(&moments),
            [
                (MomentKind::Joined, "Max"),
                (MomentKind::ShareStarted, "Anna"),
                (MomentKind::RecordingStarted, "Anna"),
                (MomentKind::TranscriptStarted, "Anna"),
                (MomentKind::ShareStopped, "Anna"),
                (MomentKind::Left, "Max"),
            ]
        );
    }

    #[test]
    fn transcript_lines_replace_and_take_back() {
        let mut log = Log::begin("me-1".into(), "Nikita".into());
        log.joined("anna-1", "Anna");
        log.said("anna-1:1", "anna-1", 10_000, "Hello");
        log.said("anna-1:2", "me-1", 12_000, "Hi Anna");
        log.said("anna-1:3", "anna-1", 13_000, "Hmm");
        // A later final replaces the first; the third was noise.
        log.said("anna-1:1", "anna-1", 10_000, "Hello there");
        log.said("anna-1:3", "anna-1", 13_000, "");
        let (_, moments) = log.finish("room", "");
        let said: Vec<_> = moments
            .iter()
            .map(|m| (m.who.as_str(), m.text.as_str(), m.mine))
            .collect();
        assert_eq!(
            said,
            [("Anna", "Hello there", false), ("Nikita", "Hi Anna", true)]
        );
    }
}
