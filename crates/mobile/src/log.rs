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
/// The most transcript lines kept for one call (a long meeting's worth),
/// and the most of one line: a call's history has a size it can't pass,
/// whatever the other end sends.
const MAX_LINES: usize = 2_000;
const MAX_LINE: usize = 1_000;
/// The most comings, goings and the like kept for one call.
const MAX_NOTES: usize = 1_000;

pub(crate) struct Log {
    /// When you got in: on the wall clock (unix ms), and for measuring.
    began: u64,
    since: Instant,
    /// Your identities in the call (a new one each time it joins again),
    /// and what you're called.
    you: Vec<String>,
    your_name: String,
    names: HashMap<String, String>,
    /// Shared screens showing now: track → whose.
    screens: HashMap<String, String>,
    people: Vec<String>,
    moments: Vec<Moment>,
    lines: usize,
    notes: usize,
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
            you: vec![you],
            your_name,
            names: HashMap::new(),
            screens: HashMap::new(),
            people: Vec::new(),
            moments: Vec::new(),
            lines: 0,
            notes: 0,
        }
    }

    fn name(&self, identity: &str) -> String {
        if self.is_you(identity) {
            return self.your_name.clone();
        }
        self.names
            .get(identity)
            .cloned()
            .unwrap_or_else(|| "Someone".into())
    }

    fn note(&mut self, kind: MomentKind, identity: &str) {
        if self.notes == MAX_NOTES {
            return;
        }
        self.notes += 1;
        self.moments.push(Moment {
            at: now() / 1000,
            kind,
            who: self.name(identity),
            mine: self.is_you(identity),
            text: String::new(),
            key: String::new(),
        });
    }

    fn is_you(&self, identity: &str) -> bool {
        self.you.iter().any(|you| you == identity)
    }

    /// Back in the call as `you`, after the connection was lost.
    pub(crate) fn rejoined(&mut self, you: String) {
        self.you.push(you);
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
        let text = match text.char_indices().nth(MAX_LINE) {
            Some((end, _)) => &text[..end],
            None => text,
        };
        match same {
            // The line as it stands now: its words, and whose and when too
            // (a correction can change those).
            Some(at) => {
                let (who, mine) = (self.name(speaker), self.is_you(speaker));
                let moment = &mut self.moments[at];
                moment.text = text.to_owned();
                moment.who = who;
                moment.mine = mine;
                moment.at = at_ms / 1000;
            }
            None if self.lines < MAX_LINES => {
                self.lines += 1;
                self.moments.push(Moment {
                    at: at_ms / 1000,
                    kind: MomentKind::Said,
                    who: self.name(speaker),
                    mine: self.is_you(speaker),
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
    fn a_call_history_has_a_size_it_cannot_pass() {
        let mut log = Log::begin("me-1".into(), "Nikita".into());
        log.joined("anna-1", "Anna");
        // A line as long as someone cares to send is cut, on a character.
        log.said("anna-1:1", "anna-1", 1000, &"é".repeat(MAX_LINE + 50));
        // Someone's recording flickering on and off all call long.
        for _ in 0..MAX_NOTES {
            log.recording("anna-1", true);
            log.recording("anna-1", false);
        }
        let (_, moments) = log.finish("room", "Team");
        let said = moments.iter().find(|m| m.kind == MomentKind::Said).unwrap();
        assert_eq!(said.text.chars().count(), MAX_LINE);
        assert_eq!(moments.len(), MAX_NOTES + 1);
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

    #[test]
    fn a_corrected_line_is_whose_and_when_the_correction_says() {
        let mut log = Log::begin("me-1".into(), "Nikita".into());
        log.joined("anna-1", "Anna");
        log.said("anna-1:1", "anna-1", 10_000, "Hello");
        log.said("anna-1:1", "me-1", 9_000, "Hello Anna");
        let (_, moments) = log.finish("room", "");
        assert_eq!(moments.len(), 1);
        let line = &moments[0];
        assert_eq!((line.who.as_str(), line.mine, line.at), ("Nikita", true, 9));
        assert_eq!(line.text, "Hello Anna");
    }
}
