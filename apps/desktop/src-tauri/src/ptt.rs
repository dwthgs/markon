//! Push-to-talk on one key: hold it while talking, or tap it and talk hands-free until a pause.
use std::time::{Duration, Instant};

/// A press shorter than this is a tap: keep listening after the key comes up.
pub const TAP: Duration = Duration::from_millis(350);
/// After a tap, give up if no speech starts within this long.
pub const NO_SPEECH: Duration = Duration::from_secs(8);
/// After Markon speaks, keep listening this long for a reply without the key.
pub const FOLLOW_UP: Duration = Duration::from_secs(6);
/// Wait this long after playback ends before listening, so the room's echo of Markon is gone.
pub const ECHO_TAIL: Duration = Duration::from_millis(300);

/// "Hey Markon, stop": words that end what Markon is doing instead of starting a request.
pub fn is_stop(text: &str) -> bool {
    let t: String = text
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect();
    matches!(
        t.split_whitespace().collect::<Vec<_>>().join(" ").as_str(),
        "stop"
            | "stop it"
            | "stop that"
            | "cancel"
            | "cancel that"
            | "never mind"
            | "nevermind"
            | "enough"
            | "quiet"
            | "be quiet"
            | "shut up"
            | "hold on"
            | "wait"
    )
}

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    /// Start listening (a new session).
    Listen(u64),
    /// Stop listening, transcribe and send.
    Finish,
    /// Stop listening and drop the audio: nobody spoke.
    Abandon,
    Nothing,
}

#[derive(Default)]
pub struct Ptt {
    down_at: Option<Instant>,
    latched: bool,
    heard: bool,
    swallow_release: bool,
    session: u64,
    /// The current session was started with the key (not by voice or after a reply).
    by_key: bool,
    /// After a reply: keep listening hands-free until then, even through noises that hold no words.
    window_end: Option<Instant>,
}

impl Ptt {
    pub fn pressed(&mut self, now: Instant) -> Action {
        if self.latched && !self.heard {
            // Listening hands-free but nothing said yet: this press starts a normal hold or tap.
            self.latched = false;
        }
        if self.latched {
            // A second tap ends hands-free listening; its release is not a new turn.
            self.latched = false;
            self.swallow_release = true;
            return Action::Finish;
        }
        self.down_at = Some(now);
        self.heard = false;
        self.by_key = true;
        self.window_end = None;
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

    /// Listen hands-free without the key: after Markon speaks, or after "Hey Markon".
    /// `None` while the key is down or a hands-free session is already open.
    pub fn follow_up(&mut self) -> Option<u64> {
        if self.down_at.is_some() || self.latched {
            return None;
        }
        self.latched = true;
        self.heard = false;
        self.by_key = false;
        self.session += 1;
        Some(self.session)
    }

    /// Listen hands-free for `len` after a reply.
    pub fn open_window(&mut self, now: Instant, len: Duration) {
        self.window_end = Some(now + len);
    }

    pub fn close_window(&mut self) {
        self.window_end = None;
    }

    /// Time left in the after-reply window, if this is a hands-free session inside one.
    pub fn window_left(&self, now: Instant) -> Option<Duration> {
        if self.by_key {
            return None;
        }
        self.window_end
            .and_then(|end| end.checked_duration_since(now))
            .filter(|d| *d >= Duration::from_secs(1))
    }

    pub fn by_key(&self) -> bool {
        self.by_key
    }

    /// `session` is still the open hands-free session (nothing newer replaced it).
    pub fn is_current(&self, session: u64) -> bool {
        self.latched && self.session == session
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
        if self.latched && !self.heard && session == self.session {
            self.latched = false;
            Action::Abandon
        } else {
            Action::Nothing
        }
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
        p.speech_started();
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
        assert_eq!(p.timed_out(s), Action::Abandon);
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

    #[test]
    fn after_a_reply_it_listens_for_a_follow_up() {
        let mut p = Ptt::default();
        let s = p.follow_up().expect("listens");
        p.speech_started();
        assert_eq!(p.speech_ended(), Action::Finish);
        let s2 = p.follow_up().expect("listens again");
        assert!(s2 > s);
        assert_eq!(p.timed_out(s2), Action::Abandon, "silence ends it quietly");
    }

    #[test]
    fn pressing_during_a_silent_follow_up_starts_a_normal_hold() {
        let mut p = Ptt::default();
        let t = Instant::now();
        p.follow_up();
        assert!(matches!(p.pressed(t), Action::Listen(_)));
        assert_eq!(p.released(t + Duration::from_secs(2)), Action::Finish);
    }

    #[test]
    fn no_follow_up_while_the_key_is_held() {
        let mut p = Ptt::default();
        p.pressed(Instant::now());
        assert_eq!(p.follow_up(), None);
    }

    #[test]
    fn the_after_reply_window_survives_a_wordless_noise() {
        let mut p = Ptt::default();
        let t = Instant::now();
        p.open_window(t, FOLLOW_UP);
        let s = p.follow_up().unwrap();
        assert!(p.is_current(s));
        // The echo of Markon's voice ends the first attempt with no words: time is left.
        p.speech_started();
        assert_eq!(p.speech_ended(), Action::Finish);
        assert!(!p.by_key());
        let left = p.window_left(t + Duration::from_secs(2)).unwrap();
        assert_eq!(left, FOLLOW_UP - Duration::from_secs(2));
        assert_eq!(p.window_left(t + FOLLOW_UP), None, "window over");
    }

    #[test]
    fn a_key_press_ends_the_window_and_owns_the_session() {
        let mut p = Ptt::default();
        let t = Instant::now();
        p.open_window(t, FOLLOW_UP);
        let s = p.follow_up().unwrap();
        assert!(matches!(p.pressed(t), Action::Listen(_)));
        assert!(
            !p.is_current(s),
            "the press replaced the hands-free session"
        );
        assert!(p.by_key());
        assert_eq!(p.window_left(t), None);
    }

    #[test]
    fn stop_words_stop_and_requests_do_not() {
        assert!(is_stop("Stop."));
        assert!(is_stop("never mind"));
        assert!(is_stop("Cancel that!"));
        assert!(!is_stop("stop the music on Spotify"));
        assert!(!is_stop(""));
    }
}
