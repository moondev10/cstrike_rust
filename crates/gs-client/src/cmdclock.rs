//! Which host frames make a user command (deviation, docs/deviations.md). Up to the classic
//! `fps_max 101` every frame makes one, as `CL_Move` does. Faster frames share commands: one
//! every 1/101.5 s, its `msec` the time it covers. The server sees the commands of a client
//! running at that rate: their count, their time adding up to the real time, and every press.
//! The original instead fills its 64-command ring within a ping at high frame rates and loses
//! prediction and delta updates. The view predicts the time waiting for the next command
//! ([`crate::Client::send`]), so it moves every frame.

/// The frame rate of `fps_max 101` (`Host_FilterTime` lets frames through at `fps_max + 0.5`).
pub const MAX_COMMAND_RATE: f64 = 101.5;
const INTERVAL: f64 = 1.0 / MAX_COMMAND_RATE;
/// A frame this close to the interval still makes its own command (the limiter's jitter at
/// the classic cap).
const SLACK: f64 = 0.0005;

/// What a host frame does about the user command.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CommandFrame {
    /// The command it makes, covering this many seconds (`None`: the next command covers it).
    pub make: Option<f64>,
    /// The seconds after the last command made, waiting for the next one.
    pub waiting: f64,
}

impl CommandFrame {
    /// A frame making its own command, as every frame of the original does.
    pub fn own(frametime: f64) -> Self {
        Self { make: Some(frametime), waiting: 0.0 }
    }
}

/// The frame time waiting for the next command.
#[derive(Debug, Default, Clone, Copy)]
pub struct CommandClock {
    pending: f64,
}

impl CommandClock {
    /// A host frame of `frametime` seconds.
    pub fn frame(&mut self, frametime: f64) -> CommandFrame {
        self.pending += frametime;
        // A frame as long as the interval makes its own command with all the time waiting; a
        // shorter one makes one each interval, which leaves less than an interval waiting.
        let covered = if frametime + SLACK >= INTERVAL {
            self.pending
        } else if self.pending >= INTERVAL {
            INTERVAL
        } else {
            return CommandFrame { make: None, waiting: self.pending };
        };
        self.pending -= covered;
        CommandFrame { make: Some(covered), waiting: self.pending }
    }

    /// Every frame makes its own command (out of a game, or a frame that must send).
    pub fn reset(&mut self) {
        self.pending = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ten seconds at `fps`: the commands made and the time they cover.
    fn run(fps: f64) -> (usize, f64, CommandClock) {
        let mut clock = CommandClock::default();
        let (mut count, mut covered) = (0, 0.0);
        for _ in 0..(fps * 10.0) as usize {
            let f = clock.frame(1.0 / fps);
            assert!(f.waiting < INTERVAL, "{fps} fps: {} waiting", f.waiting);
            if let Some(t) = f.make {
                assert!(t <= (1.0 / fps).max(INTERVAL) + 1e-9, "{fps} fps: a command of {t}");
                count += 1;
                covered += t;
            }
        }
        (count, covered, clock)
    }

    #[test]
    fn frames_up_to_the_classic_rate_each_make_a_command_and_faster_ones_share() {
        for fps in [30.0, 60.0, 100.0, 101.5] {
            let (count, covered, _) = run(fps);
            assert_eq!(count, (fps * 10.0) as usize, "{fps} fps");
            assert!((covered - 10.0).abs() < 1e-6, "{fps} fps: {covered}");
        }
        for fps in [144.0, 240.0, 500.0, 1000.0] {
            let (count, covered, clock) = run(fps);
            // The classic rate; the time not yet in a command is all that is missing.
            assert!((1014..=1016).contains(&count), "{fps} fps: {count} commands");
            assert!((covered + clock.pending - 10.0).abs() < 1e-6, "{fps} fps: {covered} + {}", clock.pending);
        }
    }

    #[test]
    fn a_drop_to_a_low_rate_makes_one_command_a_frame_at_once() {
        let mut clock = CommandClock::default();
        // 500 fps: one command after 10 ms, then some time waiting.
        let fast: Vec<CommandFrame> = (0..7).map(|_| clock.frame(0.002)).collect();
        assert_eq!(fast.iter().filter(|f| f.make.is_some()).count(), 1);
        let waiting = clock.pending;
        assert!(waiting > 0.0);
        // 30 fps: the first slow frame's single command takes the waiting time too, and every
        // later one its own frame, as the original makes them.
        assert_eq!(clock.frame(1.0 / 30.0), CommandFrame { make: Some(waiting + 1.0 / 30.0), waiting: 0.0 });
        assert_eq!(clock.frame(1.0 / 30.0), CommandFrame::own(1.0 / 30.0));
        // Back to 500 fps: commands share frames again from nothing waiting.
        assert_eq!(clock.frame(0.002), CommandFrame { make: None, waiting: 0.002 });
    }
}
