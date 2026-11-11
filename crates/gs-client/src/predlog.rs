//! `cl_predlog` (not in the original; docs/deviations.md "Diagnostics"): a text log next to the
//! executable of what the client's prediction did and what the server answered, to find where
//! the two movements part.
//!
//! - `CHECK`: a packet's acknowledged command. The server's state after it against the
//!   prediction of it, field by field, and the origin error split into the part along the
//!   predicted velocity (as a time offset: a command run for a different `msec`) and the rest.
//!   When something differs, the move traces of every command the prediction ran from the
//!   previous acknowledgement up to this one follow (each step of `PM_Move`).
//! - `CMD`: a new command's first run, its input and the state before and after.
//! - `VIEW`: a frame's simulated origin, the error smoothing pulling it and the view origin.

use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufWriter, Write as _};
use std::path::PathBuf;

use gs_protocol::structs::UserCmd;

use crate::predict::{ClientFields, LocalState};

const FL_ONGROUND: i32 = 0x200;
const FL_DUCKING: i32 = 0x4000;
const UPDATE_MASK: u32 = 63;
/// Commands dumped at most per check (the unacknowledged window is far shorter).
const MAX_CHAIN: u32 = 32;

/// A vector with three decimals.
struct V(pub [f32; 3]);

impl std::fmt::Display for V {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({:.3} {:.3} {:.3})", self.0[0], self.0[1], self.0[2])
    }
}

fn hspeed(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1]).sqrt()
}

fn ground(flags: i32) -> &'static str {
    match (flags & FL_ONGROUND != 0, flags & FL_DUCKING != 0) {
        (true, true) => "ground+duck",
        (true, false) => "ground",
        (false, true) => "air+duck",
        (false, false) => "air",
    }
}

/// One command slot's last run: the command and its move trace.
#[derive(Default)]
struct MoveRecord {
    seq: u32,
    cmd: UserCmd,
    trace: String,
    valid: bool,
}

/// The server's view of the local player after an acknowledged command.
pub(crate) struct ServerState {
    pub client: ClientFields,
    /// `usehull`, `basevelocity`, `friction`, `gravity`, `movetype` of the player's entity.
    pub usehull: Option<i32>,
    pub basevelocity: Option<[f32; 3]>,
    pub friction: Option<f32>,
    pub gravity: Option<f32>,
    pub movetype: Option<i32>,
    /// The frame's server time.
    pub time: f64,
}

pub(crate) struct PredLog {
    out: BufWriter<File>,
    line: String,
    moves: Vec<MoveRecord>,
    /// The acknowledgement of the previous packet check: the state the runs up to the next one
    /// started from.
    last_ack: Option<u32>,
    last_flush: f64,
    pub path: PathBuf,
}

impl PredLog {
    /// `predlog.txt` next to the executable, truncated.
    pub fn open() -> Option<Self> {
        let path = std::env::current_exe().ok()?.parent()?.join("predlog.txt");
        let file = File::create(&path).map_err(|e| log::warn!("{}: {e}", path.display())).ok()?;
        let mut log = Self { out: BufWriter::new(file), line: String::new(), moves: (0..64).map(|_| MoveRecord::default()).collect(), last_ack: None, last_flush: 0.0, path };
        log.write(
            "# cl_predlog. Times are the client's realtime (s). CHECK: server state after the acknowledged command vs the \
             prediction of it; err = server - predicted origin, `along vel` as the time the command would have run longer \
             (ms), `across` the rest. Tolerances follow delta.lst's precision (origin 1/128, velocity 1/8, view_ofs 1/4, \
             fuser2 and timers whole). CMD: a new command's first run. VIEW: per frame, sim origin, smoothing keep \
             (1 = none), view origin.\n",
        );
        Some(log)
    }

    fn write(&mut self, text: &str) {
        if let Err(e) = self.out.write_all(text.as_bytes()) {
            log::warn!("{}: {e}", self.path.display());
        }
    }

    fn flush_line(&mut self) {
        let line = std::mem::take(&mut self.line);
        self.write(&line);
        self.line = line;
        self.line.clear();
    }

    fn maybe_flush(&mut self, now: f64) {
        if now - self.last_flush >= 1.0 || now < self.last_flush {
            self.last_flush = now;
            let _ = self.out.flush();
        }
    }

    /// The trace buffer of `slot` for a new run (its old text cleared).
    pub fn take_trace(&mut self, slot: usize) -> String {
        let mut s = std::mem::take(&mut self.moves[slot].trace);
        s.clear();
        s
    }

    /// Keeps a run's command and move trace in its slot.
    pub fn store_move(&mut self, slot: usize, seq: u32, cmd: &UserCmd, trace: String) {
        self.moves[slot] = MoveRecord { seq, cmd: *cmd, trace, valid: true };
    }

    /// Every `svc_newmovevars` (what the prediction moves with from then on).
    pub fn movevars(&mut self, now: f64, mv: &gs_protocol::messages::MoveVars) {
        let _ = writeln!(
            self.line,
            "{now:.4} MOVEVARS gravity {} stopspeed {} maxspeed {} spectatormaxspeed {} accelerate {} airaccelerate {} wateraccelerate {} friction {} edgefriction {} waterfriction {} entgravity {} bounce {} stepsize {} maxvelocity {} footsteps {}",
            mv.gravity,
            mv.stopspeed,
            mv.maxspeed,
            mv.spectatormaxspeed,
            mv.accelerate,
            mv.airaccelerate,
            mv.wateraccelerate,
            mv.friction,
            mv.edgefriction,
            mv.waterfriction,
            mv.entgravity,
            mv.bounce,
            mv.stepsize,
            mv.maxvelocity,
            mv.footsteps,
        );
        self.flush_line();
        let _ = self.out.flush();
    }

    /// A free-form line (connection events).
    pub fn note(&mut self, now: f64, text: &str) {
        let _ = writeln!(self.line, "{now:.4} {text}");
        self.flush_line();
        let _ = self.out.flush();
    }

    /// A new command's first run.
    pub fn cmd(&mut self, now: f64, seq: u32, cmd: &UserCmd, from: &LocalState, to: &LocalState) {
        let (b, a) = (&from.client, &to.client);
        let mut events = String::new();
        if b.flags & FL_ONGROUND != 0 && a.flags & FL_ONGROUND == 0 && a.velocity[2] > 0.0 {
            events.push_str(" JUMP");
        }
        if b.flags & FL_ONGROUND == 0 && a.flags & FL_ONGROUND != 0 {
            events.push_str(" LAND");
        }
        let _ = writeln!(
            self.line,
            "{now:.4} CMD seq {seq} msec {} btn {:#06x} move {:.1} {:.1} {:.1} ang ({:.3} {:.3}) | {} org {} vel {} hspd {:.3} -> {} org {} vel {} hspd {:.3} fuser2 {:.1}{events}",
            cmd.msec,
            cmd.buttons,
            cmd.forwardmove,
            cmd.sidemove,
            cmd.upmove,
            cmd.viewangles[0],
            cmd.viewangles[1],
            ground(b.flags),
            V(from.player.origin),
            V(b.velocity),
            hspeed(b.velocity),
            ground(a.flags),
            V(to.player.origin),
            V(a.velocity),
            hspeed(a.velocity),
            a.fuser[1],
        );
        self.flush_line();
        self.maybe_flush(now);
    }

    /// A packet's check of the acknowledged command `ack`: `predicted` is the state the
    /// prediction gave it, `predicted_origin` what `cl_showerror` compares.
    #[allow(clippy::too_many_arguments)]
    pub fn check(&mut self, now: f64, phase: &str, seq: u32, ack: u32, outgoing: u32, server: &ServerState, predicted: &LocalState, predicted_origin: [f32; 3]) {
        let s = &server.client;
        let p = &predicted.client;
        let err = [0, 1, 2].map(|k| s.origin[k] - predicted_origin[k]);
        let len = (err[0] * err[0] + err[1] * err[1] + err[2] * err[2]).sqrt();
        // The error as a time offset along the predicted velocity, and what is left across it.
        let v = p.velocity;
        let vv = v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
        let (along_ms, across) = if vv > 1.0 {
            let t = (err[0] * v[0] + err[1] * v[1] + err[2] * v[2]) / vv;
            let rest = [0, 1, 2].map(|k| err[k] - v[k] * t);
            (t * 1000.0, (rest[0] * rest[0] + rest[1] * rest[1] + rest[2] * rest[2]).sqrt())
        } else {
            (0.0, len)
        };
        let mut diffs = String::new();
        let mut diff = |name: &str, srv: String, pred: String| {
            let _ = write!(diffs, " {name} srv {srv} pred {pred};");
        };
        let far = |a: f32, b: f32, tol: f32| (a - b).abs() > tol;
        if (0..3).any(|k| far(s.origin[k], predicted.player.origin[k], 0.01)) {
            diff("origin", V(s.origin).to_string(), V(predicted.player.origin).to_string());
        }
        // Two steps of delta.lst's 1/8: the server truncates toward zero, the seed repeats it.
        if (0..3).any(|k| far(s.velocity[k], p.velocity[k], 0.26)) {
            diff("velocity", V(s.velocity).to_string(), V(p.velocity).to_string());
        }
        if s.flags & (FL_ONGROUND | FL_DUCKING) != p.flags & (FL_ONGROUND | FL_DUCKING) {
            diff("flags", ground(s.flags).into(), ground(p.flags).into());
        }
        if s.in_duck != p.in_duck {
            diff("bInDuck", s.in_duck.to_string(), p.in_duck.to_string());
        }
        if (s.duck_time - p.duck_time).abs() > 1 {
            diff("flDuckTime", s.duck_time.to_string(), p.duck_time.to_string());
        }
        if (s.time_step_sound - p.time_step_sound).abs() > 1 {
            diff("flTimeStepSound", s.time_step_sound.to_string(), p.time_step_sound.to_string());
        }
        if far(s.fuser[1], p.fuser[1], 1.0) {
            diff("fuser2", format!("{:.3}", s.fuser[1]), format!("{:.3}", p.fuser[1]));
        }
        if far(s.maxspeed, p.maxspeed, 0.11) {
            diff("maxspeed", format!("{:.2}", s.maxspeed), format!("{:.2}", p.maxspeed));
        }
        if far(s.view_ofs[2], p.view_ofs[2], 0.26) {
            diff("view_ofs.z", format!("{:.2}", s.view_ofs[2]), format!("{:.2}", p.view_ofs[2]));
        }
        if s.waterlevel != p.waterlevel {
            diff("waterlevel", s.waterlevel.to_string(), p.waterlevel.to_string());
        }
        if far(s.punchangle[0], p.punchangle[0], 0.01) || far(s.punchangle[1], p.punchangle[1], 0.01) || far(s.punchangle[2], p.punchangle[2], 0.13) {
            diff("punchangle", V(s.punchangle).to_string(), V(p.punchangle).to_string());
        }
        if server.usehull.is_some_and(|h| h != predicted.player.usehull) {
            diff("usehull", format!("{:?}", server.usehull), predicted.player.usehull.to_string());
        }
        if server.basevelocity.is_some_and(|b| (0..3).any(|k| far(b[k], predicted.player.basevelocity[k], 0.13))) {
            diff("basevelocity", format!("{:?}", server.basevelocity), V(predicted.player.basevelocity).to_string());
        }
        if server.friction.is_some_and(|f| far(f, predicted.player.friction, 0.01)) {
            diff("friction", format!("{:?}", server.friction), predicted.player.friction.to_string());
        }
        if server.gravity.is_some_and(|g| far(g, predicted.player.gravity, 0.01)) {
            diff("gravity", format!("{:?}", server.gravity), predicted.player.gravity.to_string());
        }
        if server.movetype.is_some_and(|m| m != predicted.player.movetype) {
            diff("movetype", format!("{:?}", server.movetype), predicted.player.movetype.to_string());
        }
        let pending = outgoing.wrapping_sub(ack).wrapping_sub(1);
        let _ = writeln!(
            self.line,
            "{now:.4} CHECK {phase} seq {seq} ack {ack} pending {pending} srvtime {:.4} err {len:.3} {} along vel {along_ms:+.3} ms across {across:.3} | srv {} vel {} hspd {:.3} | pred {} vel {} hspd {:.3}",
            server.time,
            V(err),
            ground(s.flags),
            V(s.velocity),
            hspeed(s.velocity),
            ground(p.flags),
            V(p.velocity),
            hspeed(p.velocity),
        );
        let interesting = len > 0.05 || !diffs.is_empty();
        if !diffs.is_empty() {
            let _ = writeln!(self.line, "  DIFF{diffs}");
        }
        // The runs that led to this prediction, from the previous packet's acknowledgement (once
        // per packet: the re-prediction's check repeats the same comparison).
        if interesting && phase == "packet" {
            let first = self.last_ack.map_or(ack, |a| a.wrapping_add(1));
            let count = ack.wrapping_sub(first).wrapping_add(1).min(MAX_CHAIN);
            let _ = writeln!(self.line, "  runs from the server state of ack {}:", first.wrapping_sub(1));
            for n in 0..count {
                let seq = ack.wrapping_sub(count - 1 - n);
                let slot = (seq & UPDATE_MASK) as usize;
                let m = &self.moves[slot];
                if !m.valid || m.seq != seq {
                    let _ = writeln!(self.line, "    cmd {seq}: no trace (not run since logging started)");
                    continue;
                }
                let c = &m.cmd;
                let _ = writeln!(
                    self.line,
                    "    cmd {seq}: msec {} btn {:#06x} move {:.1} {:.1} {:.1} ang ({:.3} {:.3} {:.3})",
                    c.msec, c.buttons, c.forwardmove, c.sidemove, c.upmove, c.viewangles[0], c.viewangles[1], c.viewangles[2]
                );
                self.line.push_str(&m.trace);
            }
        }
        if phase == "packet" {
            self.last_ack = Some(ack);
        }
        self.flush_line();
        if interesting {
            let _ = self.out.flush();
        }
        self.maybe_flush(now);
    }

    /// A frame: the simulated origin (`raw` before smoothing, `keep` the share of the move the
    /// smoothing let through, 1 without smoothing) and the view origin.
    #[allow(clippy::too_many_arguments)]
    pub fn view(&mut self, now: f64, frametime: f64, sim: [f32; 3], raw: [f32; 3], keep: f32, onground: i32, viewheight: f32, vieworg: [f32; 3]) {
        let _ = writeln!(
            self.line,
            "{now:.4} VIEW ft {:.2}ms sim {} raw-sim {:.3} keep {keep:.3} onground {onground} viewheight {viewheight:.2} view {} view.z-sim.z {:.3}",
            frametime * 1000.0,
            V(sim),
            ((raw[0] - sim[0]).powi(2) + (raw[1] - sim[1]).powi(2) + (raw[2] - sim[2]).powi(2)).sqrt(),
            V(vieworg),
            vieworg[2] - sim[2],
        );
        self.flush_line();
        self.maybe_flush(now);
    }
}

impl Drop for PredLog {
    fn drop(&mut self) {
        let _ = self.out.flush();
    }
}
