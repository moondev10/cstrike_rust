//! The outgoing side (docs/re/net-transport.md §8): `CL_RegisterResources`' sign-on answer,
//! `CL_Move` with its rate, interpolation and packet-loss bookkeeping, prediction and the
//! `clc_move`, `net_showpackets`' outgoing line, overflows, and the cvars `CL_Move` writes.

use std::sync::Arc;

use gs_formats::bsp::{self, Bsp};
use gs_net::munge::{MungeTable, munge};
use gs_protocol::delta::names;
use gs_protocol::state::UPDATE_MASK;
use gs_protocol::structs::UserCmd;
use gs_protocol::{MsgWriter, clc};
use gs_world::World;

use super::{
    BUILD_WITH_MORE_COMMANDS, CMD_SLOTS, Client, ClientEvent, CmdSlot, LEGACY_MAX_NEW_COMMANDS, MOVE_MESSAGE_SIZE, State, UiEvent, voice_into_datagram,
};
use crate::postrun::PostRunCmd;
use crate::predict::LocalState;
use crate::resources;
use crate::voice::VoiceSend;

impl Client {
    /// `CL_RegisterResources` into the registration buffer: load precached models, then the
    /// consistency answer and `spawn <count> <munged map crc>`.
    pub(super) fn register_resources(&mut self, w: &mut MsgWriter) {
        let Some(info) = self.server_info.clone() else { return };
        self.startup_mark("begin CL_RegisterResources()");
        self.ui(UiEvent::ContinueProgress(8));
        self.startup_mark("begin CL_PrecacheResources()");
        // One read serves the CRC and the world.
        let map = match self.fs.read(&info.map_model) {
            Ok(data) => data,
            Err(e) => return self.disconnect(&format!("Cannot continue without map {}: {e}", info.map_model)),
        };
        let crc = match bsp::map_crc(&map) {
            Ok(crc) if crc as i32 == info.map_crc || self.demo.playing() => crc,
            Ok(_) => return self.disconnect(&format!("Your map [{}] differs from the server's.", info.map_model)),
            Err(e) => return self.disconnect(&format!("Cannot continue without map {}: {e}", info.map_model)),
        };
        match Bsp::parse(&map) {
            Ok(bsp) => {
                // One loading step per `maps/*.bsp` model on hand.
                let maps = self
                    .on_hand
                    .iter()
                    .filter(|r| {
                        let name = r.name.to_ascii_lowercase();
                        r.kind == crate::resources::kind::MODEL && name.starts_with("maps/") && name.ends_with(".bsp")
                    })
                    .count();
                for _ in 0..maps {
                    self.ui(UiEvent::ContinueProgress(9));
                }
                self.level.entity_lump = bsp.entities.clone();
                self.prediction.world = Some(Arc::new(World::from_bsp(&bsp)));
                // R_NewMap: the renderer takes the world here, after the resources are in.
                self.events.push_back(ClientEvent::NewMap(info.map_model.clone()));
            }
            Err(e) => return self.disconnect(&format!("Couldn't load map {}: {e}", info.map_model)),
        }
        self.level.load_studio_models(&self.fs);
        self.startup_mark("end  CL_PrecacheResources()");
        // CL_InitSpectator: a fresh spectator state; an HLTV client's is a noclip camera.
        self.spec_state = LocalState::default();
        if self.spectator() {
            let p = &mut self.spec_state.player;
            p.number = i32::from(self.proto.playernum) + 1;
            p.friction = 1.0;
            p.gravity = 1.0;
            p.usehull = 1;
            p.movetype = 8;
            self.spec_state.client.maxspeed = self.level.movevars.spectatormaxspeed;
        }
        // CL_SendConsistencyInfo (never in a playback).
        if let Some(list) = self.resources.as_ref().filter(|l| !l.consistency.is_empty() && !self.demo.playing())
            && let Err(e) = resources::write_consistency(w, &self.fs, list, info.spawncount)
        {
            return self.disconnect(&e);
        }
        // A passive client never sends `spawn` (signon 2 comes from svc_hltv).
        if self.passive {
            return;
        }
        let mut crc_bytes = crc.to_le_bytes();
        munge(&mut crc_bytes, i32::from(!(info.spawncount as u8)), MungeTable::Two);
        clc::stringcmd(w, &format!("spawn {} {}", info.spawncount, i32::from_le_bytes(crc_bytes)));
        self.startup_mark("end   CL_RegisterResources()");
    }

    /// `CL_Move` (hw.dll 0x101a3a40): a keepalive while connected; from `ca_uninitialized` on the
    /// frame's command with the rate and interpolation checks, then (not in a playback)
    /// prediction and the `clc_move` (+`clc_delta`).
    pub(super) fn send_commands(&mut self, frametime: f64, weapons: &mut dyn PostRunCmd, voice: &mut dyn VoiceSend) {
        if self.state < State::Connected {
            return;
        }
        let fragment_size = self.fragment_size();
        let Some(chan) = self.netchan.as_mut() else { return };
        chan.fragment_size = fragment_size;
        // A passive (multicast) client never transmits.
        if self.state == State::Connected && self.passive {
            return;
        }
        if self.state == State::Connected {
            // Netchan_Transmit(0); a playback's channel has no remote to send to.
            let packet = chan.transmit(&[], self.now);
            match (packet, self.server) {
                (Ok(p), Some(server)) => {
                    self.send_raw(server, &p);
                    self.trace_outgoing(&p);
                }
                (Ok(p), None) => self.trace_outgoing(&p),
                (Err(e), _) => self.outgoing_overflow(&e),
            }
            return;
        }
        // The packet-loss estimate, once a second: dropped frames of the last 64.
        if self.now >= self.packet_loss_recalc {
            self.packet_loss_recalc = self.now + 1.0;
            let incoming = chan.incoming_sequence;
            let lost = (0..CMD_SLOTS as u32).filter(|i| self.frame_received[(incoming.wrapping_sub(*i) & UPDATE_MASK) as usize] == -1.0).count();
            self.packet_loss = (f64::from(lost as f32) * 100.0 / f64::from(64.0f32)) as f32;
        }
        let sequence = chan.outgoing_sequence;

        // cl_updaterate within [10, 102], written back.
        if self.config.updaterate < 10.0 {
            self.engine_cvar_set("cl_updaterate", "20");
            self.print("cl_updaterate minimum is 10.000000, resetting to default (20)\n".to_owned());
        }
        if self.config.updaterate > 102.0 {
            self.engine_cvar_set("cl_updaterate", "102");
            self.print("cl_updaterate maximum is 102.000000, resetting to maximum (102)\n".to_owned());
        }
        // ex_interp (whole milliseconds) within one update interval .. 100 ms (200 on an HLTV
        // connection), written back with a console line.
        let updaterate = self.config.updaterate;
        let max_ms: i32 = if self.proto.spectator { 200 } else { 100 };
        let min_ms: i32 = if updaterate <= 0.0 { 50 } else { ((1000.0 / f64::from(updaterate)) as f32 as i32).max(1) };
        let mut ms = (f64::from(self.ex_interp as f32) * 1000.0) as f32 as i32;
        let forced = if ms.wrapping_add(1) < min_ms {
            ms = min_ms;
            Some(format!("ex_interp forced up to {min_ms} msec\n"))
        } else if ms.wrapping_sub(1) > max_ms {
            ms = max_ms;
            Some(format!("ex_interp forced down to {max_ms} msec\n"))
        } else {
            None
        };
        if let Some(line) = forced {
            self.print(line);
            let value = format!("{:.6}", f64::from((f64::from(ms as f32) / 1000.0) as f32));
            self.engine_cvar_set("ex_interp", &value);
        }
        let ms = ms.max(min_ms).min(max_ms);
        // cl_lerp_smoothed slews toward the target by at most 5 % of the frame time (floats).
        let target = (f64::from(ms as f32) / 1000.0) as f32;
        if target != self.lerp {
            let diff = target - self.lerp;
            let step = (frametime * f64::from(0.05f32)) as f32;
            self.lerp += if diff <= 0.0 { -step.min(-diff) } else { step.min(diff) };
        }
        let lerp_msec = ((f64::from(self.lerp) * 1000.0) as f32 as i32).clamp(0, 100) as i16;

        let ms = frametime * 1000.0;
        let mut msec = ms as i32;
        self.msec_carry += ms - f64::from(msec);
        if self.msec_carry > 1.0 {
            msec += 1;
            self.msec_carry -= 1.0;
        }
        let msec = f64::from(msec).min(255.0) as i32 as u8;
        self.cl_cmd = UserCmd { msec, lerp_msec, lightlevel: self.light_level as u8, ..self.next_cmd };
        self.commands[(sequence & UPDATE_MASK) as usize] = CmdSlot { cmd: self.cl_cmd, senttime: self.now as f32, ..CmdSlot::default() };
        // Neither prediction nor sending in a playback or while passive.
        if self.demo.playing() || self.passive {
            return;
        }
        self.run_prediction(false, weapons);
        if self.config.cmdrate < 10.0 {
            self.engine_cvar_set("cl_cmdrate", "10");
        }

        // HLTV idle gate: nothing at all (no hold-back, no sequence) while the delta is
        // acknowledged and less than a second has passed since the next allowed send.
        let spectator = self.spectator();
        if spectator
            && self.state == State::Active
            && self.delta_sequence == Some(self.proto.valid_sequence)
            && !(self.demo.recording() && self.demo.waiting)
            && f64::from((f64::from(self.next_cmd_time) + 1.0) as f32) > self.now
        {
            return;
        }
        let Some(server) = self.server else { return };
        let usercmd_delta = self.proto.delta(names::USERCMD).cloned();
        let Some(chan) = self.netchan.as_mut() else { return };
        // A single-player game sends every frame; otherwise cl_cmdrate and the channel rate gate.
        let due = self.proto.maxclients == 1 || (self.now >= f64::from(self.next_cmd_time) && chan.can_packet(self.now));
        let Some(usercmd_delta) = usercmd_delta.filter(|_| msec > 0 && due) else {
            self.commands[(sequence & UPDATE_MASK) as usize].heldback = true;
            chan.skip_sequence();
            return;
        };
        let cmdrate = self.config.cmdrate;
        self.next_cmd_time = if cmdrate <= 0.0 { self.now as f32 } else { (f64::from((1.0 / f64::from(cmdrate)) as f32) + self.now) as f32 };
        // cls.lastoutgoingcommand starts at -1, which the first send turns into this sequence.
        let last = *self.last_sent_sequence.get_or_insert(sequence);
        let max_new = if self.build_num.unwrap_or(0) >= BUILD_WITH_MORE_COMMANDS {
            clc::MAX_MOVE_COMMANDS - usize::from(self.config.cmdbackup)
        } else {
            LEGACY_MAX_NEW_COMMANDS
        };
        let new = (sequence.wrapping_sub(last) as usize).min(max_new);
        let backup = self.config.cmdbackup.min(8);
        let total = usize::from(backup) + new;
        let cmds: Vec<UserCmd> =
            (0..total).rev().map(|i| self.commands[(sequence.wrapping_sub(i as u32) & UPDATE_MASK) as usize].cmd).collect();
        let mut w = MsgWriter::new();
        let loss = (self.packet_loss as i32).clamp(0, 100) as u8 | if voice.loopback() { 0x80 } else { 0 };
        // An HLTV spectator sends no clc_move at all (SZ_Clear of the whole body).
        if !spectator {
            clc::write_move(&mut w, &usercmd_delta, &cmds, backup, loss, sequence);
        }
        // No acknowledgement for 63 commands and nothing heard for 15 s: the on-screen warning
        // and a full update; otherwise the delta request.
        self.delta_sequence = None;
        if self.state == State::Active
            && self.signon == 2
            && chan.outgoing_sequence.wrapping_sub(chan.incoming_acknowledged) >= UPDATE_MASK
            && self.now - chan.last_received > 15.0
        {
            self.connection_problem = true;
            self.proto.valid_sequence = 0;
        } else if self.state == State::Active && self.proto.valid_sequence != 0 && !(self.demo.recording() && self.demo.waiting) {
            clc::delta(&mut w, self.proto.valid_sequence);
            self.delta_sequence = Some(self.proto.valid_sequence);
        }
        self.last_sent_sequence = Some(sequence);
        self.commands[(sequence & UPDATE_MASK) as usize].sendsize = w.len() as i32;
        // CL_AddVoiceToDatagram, then cls.datagram when it fits; it is cleared either way.
        voice_into_datagram(&mut self.datagram, self.state, voice);
        if w.len() + self.datagram.len() <= MOVE_MESSAGE_SIZE {
            w.write_bytes(&self.datagram);
        }
        self.datagram.clear();
        match chan.transmit(w.as_bytes(), self.now) {
            Ok(p) => {
                self.send_raw(server, &p);
                self.trace_outgoing(&p);
            }
            Err(e) => self.outgoing_overflow(&e),
        }
    }

    /// `Netchan_Transmit`'s last statement: `net_showpackets` other than 0 and 2 prints the
    /// datagram just built (net-transport.md §9.11).
    pub(super) fn trace_outgoing(&mut self, datagram: &[u8]) {
        let show = self.config.show_packets;
        if show == 0.0 || show == 2.0 || datagram.len() < 8 {
            return;
        }
        let w1 = u32::from_le_bytes([datagram[0], datagram[1], datagram[2], datagram[3]]);
        let w2 = u32::from_le_bytes([datagram[4], datagram[5], datagram[6], datagram[7]]);
        let tm = self.time() as f32;
        self.print(format!(" c --> sz={} seq={} ack={} rel={} tm={tm:.6}\n", datagram.len(), w1 & 0x3FFF_FFFF, w2 & 0x3FFF_FFFF, w1 >> 31));
    }

    /// `Netchan_Transmit` with `FSB_OVERFLOWED`: `"%s:Outgoing message overflow"`, nothing sent
    /// (the original keeps the connection).
    fn outgoing_overflow(&mut self, e: &gs_net::NetchanError) {
        let adr = self.netchan.as_ref().map_or_else(String::new, |c| c.remote.to_string());
        log::debug!("transmit: {e}");
        self.print(format!("{adr}:Outgoing message overflow\n"));
    }

    /// `Cvar_Set` from `CL_Move`: the client's copy, the userinfo and `setinfo` for the USERINFO
    /// `cl_updaterate`, and the console's cvar through [`ClientEvent::CvarMirror`].
    pub(super) fn engine_cvar_set(&mut self, name: &str, value: &str) {
        let v = gs_console::cvar::q_atof(value);
        match name {
            "cl_updaterate" => {
                self.config.updaterate = v;
                match self.config.userinfo.set(name, value) {
                    Ok(()) => {
                        if self.state >= State::Connected {
                            self.send_string_cmd(&format!("setinfo \"{name}\" \"{value}\"\n"));
                        }
                    }
                    Err(e) => self.print(format!("{e}\n")),
                }
            }
            "ex_interp" => self.ex_interp = f64::from(v),
            "cl_cmdrate" => self.config.cmdrate = v,
            _ => {}
        }
        self.events.push_back(ClientEvent::CvarMirror { name: name.to_owned(), value: value.to_owned() });
    }
}
