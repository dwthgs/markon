//! Push-to-talk on one key: hold it while talking, or tap it and talk hands-free until a pause.
use std::time::{Duration, Instant};

/// A press shorter than this is a tap: keep listening after the key comes up.
pub const TAP: Duration = Duration::from_millis(350);
/// After a tap, give up if no speech starts within this long.
pub const NO_SPEECH: Duration = Duration::from_secs(8);

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    /// Start listening (a new session).
    Listen(u64),
    /// Stop listening, transcribe and send.
    Finish,
    Nothing,
}

#[derive(Default)]
pub struct Ptt {
    down_at: Option<Instant>,
    latched: bool,
    heard: bool,
    swallow_release: bool,
    session: u64,
}

impl Ptt {
    pub fn pressed(&mut self, now: Instant) -> Action {
        if self.latched {
            // A second tap ends hands-free listening; its release is not a new turn.
            self.latched = false;
            self.swallow_release = true;
            return Action::Finish;
        }
        self.down_at = Some(now);
        self.heard = false;
        self.session += 1;
        Action::Listen(self.session)
    }

    pub fn released(&mut self, now: Instant) -> Action {
        if std::mem::take(&mut self.swallow_release) {
            return Action::Nothing;
        }
        match self.down_at.take() {
            Some(t) if now.duration_since(t) < TAP => {
                self.latched = true;
                Action::Nothing
            }
            Some(_) => Action::Finish,
            None => Action::Nothing,
        }
    }

    pub fn speech_started(&mut self) {
        self.heard = true;
    }

    /// The voice activity detector heard a pause after speech.
    pub fn speech_ended(&mut self) -> Action {
        self.unlatch(self.latched)
    }

    /// The no-speech timer for `session` fired.
    pub fn timed_out(&mut self, session: u64) -> Action {
        self.unlatch(self.latched && !self.heard && session == self.session)
    }

    fn unlatch(&mut self, when: bool) -> Action {
        if when {
            self.latched = false;
            Action::Finish
        } else {
            Action::Nothing
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hold_talks_until_release() {
        let mut p = Ptt::default();
        let t = Instant::now();
        assert_eq!(p.pressed(t), Action::Listen(1));
        p.speech_started();
        assert_eq!(
            p.speech_ended(),
            Action::Nothing,
            "a pause while holding does not end the turn"
        );
        assert_eq!(p.released(t + Duration::from_secs(2)), Action::Finish);
    }

    #[test]
    fn tap_listens_until_a_pause() {
        let mut p = Ptt::default();
        let t = Instant::now();
        p.pressed(t);
        assert_eq!(p.released(t + Duration::from_millis(120)), Action::Nothing);
        p.speech_started();
        assert_eq!(p.speech_ended(), Action::Finish);
        assert_eq!(p.speech_ended(), Action::Nothing);
    }

    #[test]
    fn a_second_tap_ends_hands_free_listening() {
        let mut p = Ptt::default();
        let t = Instant::now();
        p.pressed(t);
        p.released(t + Duration::from_millis(100));
        assert_eq!(p.pressed(t + Duration::from_secs(3)), Action::Finish);
        assert_eq!(p.released(t + Duration::from_secs(4)), Action::Nothing);
    }

    #[test]
    fn a_tap_with_no_speech_gives_up() {
        let mut p = Ptt::default();
        let t = Instant::now();
        let Action::Listen(s) = p.pressed(t) else {
            panic!()
        };
        p.released(t + Duration::from_millis(100));
        assert_eq!(p.timed_out(s), Action::Finish);
    }

    #[test]
    fn the_timer_leaves_a_speaker_or_a_newer_session_alone() {
        let mut p = Ptt::default();
        let t = Instant::now();
        let Action::Listen(s) = p.pressed(t) else {
            panic!()
        };
        p.released(t + Duration::from_millis(100));
        p.speech_started();
        assert_eq!(p.timed_out(s), Action::Nothing);
        assert_eq!(p.timed_out(s + 1), Action::Nothing);
    }
}
