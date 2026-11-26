//! Home's player where there's no AVPlayer. Recordings are made on Macs
//! only for now (`record.rs`), so there's nothing here to play: `open`
//! finds nothing, and Home shows no player.

use std::path::Path;
use std::time::Duration;

pub enum Player {}

impl Player {
    pub fn open(_path: &Path) -> Option<Self> {
        None
    }

    pub fn path(&self) -> &Path {
        match *self {}
    }

    pub fn play(&self) {
        match *self {}
    }

    pub fn pause(&self) {
        match *self {}
    }

    pub fn is_playing(&self) -> bool {
        match *self {}
    }

    pub fn seek(&self, _at: Duration) {
        match *self {}
    }

    pub fn position(&self) -> Duration {
        match *self {}
    }

    pub fn duration(&self) -> Option<Duration> {
        match *self {}
    }
}
