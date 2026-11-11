//! `net_graph` (docs/re/engine-misc.md §8.3, `SCR_NetGraph@0x101E04F0`): the latency and
//! loss graph over the last incoming frames, the per-category byte stacks, the outgoing
//! interpolation graph and the text block. Every pixel is an additive `FillRGBA`.

use gs_client::NetStats;
use gs_console::cvar::Cvars;
use gs_draw::Screen;
use gs_protocol::state::NetBytes;

/// The ring the graph keeps between calls (`seq & 1023`).
const RING: usize = 1024;
/// Frame states beyond a latency height.
const DROPPED: i32 = 9999;
const INVALID: i32 = 9998;

/// The outgoing lerp colours (`NET_Init`, truncated floats).
const RAMP: [[i32; 3]; 24] = [
    [63, 0, 100],
    [55, 7, 119],
    [47, 15, 138],
    [39, 23, 158],
    [31, 31, 177],
    [23, 39, 196],
    [15, 47, 216],
    [7, 55, 235],
    [255, 127, 0],
    [254, 119, 0],
    [254, 111, 0],
    [254, 103, 0],
    [253, 95, 0],
    [253, 87, 0],
    [253, 79, 0],
    [252, 71, 0],
    [252, 63, 0],
    [252, 55, 0],
    [251, 47, 0],
    [251, 39, 0],
    [251, 31, 0],
    [250, 23, 0],
    [250, 15, 0],
    [250, 7, 0],
];

/// The graph's statics.
pub struct NetGraph {
    state: Vec<i32>,
    choked: Vec<bool>,
    bytes: Vec<NetBytes>,
    /// Outgoing `frame_lerp`, `sendsize`, and `heldback == 0`.
    lerp: Vec<f32>,
    size: Vec<i32>,
    sent: Vec<bool>,
    loss: f32,
    choke: f32,
    /// The text block's frame-time average and the last non-zero outgoing size.
    fps_avg: f32,
    last_out: i32,
}

impl Default for NetGraph {
    fn default() -> Self {
        Self {
            state: vec![0; RING],
            choked: vec![false; RING],
            bytes: vec![NetBytes::default(); RING],
            lerp: vec![0.0; RING],
            size: vec![0; RING],
            sent: vec![false; RING],
            loss: 0.0,
            choke: 0.0,
            fps_avg: 0.0,
            last_out: 0,
        }
    }
}

fn fill(screen: &mut Screen, x: i32, y: i32, w: i32, h: i32, c: [i32; 4]) {
    screen.fill_rgba(x, y, w, h, c[0], c[1], c[2], c[3]);
}

impl NetGraph {
    /// `SCR_NetGraph`. `graph_held` is the client's `+graph`; `frametime` `host_frametime`.
    pub fn draw(&mut self, screen: &mut Screen, cvars: &mut Cvars, stats: &NetStats, graph_held: bool, frametime: f64) {
        let mut level = cvars.value("net_graph") as i32;
        if level == 0 {
            if !graph_held {
                return;
            }
            level = 2;
        }
        if cvars.value("net_scale") <= 0.0 {
            cvars.set_value_only("net_scale", 1.0);
        }
        let scale = cvars.value("net_scale");
        let graphheight = cvars.value("graphheight");
        let (sw, sh) = (screen.width as i32, screen.height as i32);
        let mut w = (cvars.value("net_graphwidth") as i32).min(1024);
        if sw < w + 10 {
            w = sw - 10;
        }
        let gx = match cvars.value("net_graphpos") as i32 {
            1 => sw - w - 5,
            2 => (sw - w - 10) / 2,
            _ => 5,
        };
        let by = sh - 29;
        let hmax = ((graphheight - 24.0) - 2.0) as i32;

        // The refresh over the last 64 incoming frames.
        let incoming = stats.incoming_sequence;
        let (mut lost, mut choked) = (0, 0);
        let (mut ms_sum, mut ms_count) = (0.0f64, 0);
        let mut maxmsg = 0u16;
        for i in 0..64u32 {
            let seq = incoming.wrapping_sub(63).wrapping_add(i);
            let slot = (seq & 63) as usize;
            let ring = (seq as usize) & (RING - 1);
            let frame = &stats.frames[slot];
            self.choked[ring] = frame.choked;
            if frame.choked {
                choked += 1;
            }
            let received = stats.received[slot];
            self.state[ring] = if !frame.valid {
                INVALID
            } else if received == -1.0 {
                lost += 1;
                DROPPED
            } else {
                let latency = stats.latency[slot];
                (((latency.min(1.0) as f32 + 0.1) / 1.1) * hmax as f32) as i32
            };
            if self.state[ring] < 9997 && seq > incoming.wrapping_sub(8) {
                ms_sum += stats.latency[slot] * 1000.0;
                ms_count += 1;
            }
            self.bytes[ring] = frame.bytes;
            maxmsg = maxmsg.max(frame.bytes.msgbytes);
        }
        let maxmsg = maxmsg.min(1000);
        self.loss = (lost as f32 * 100.0 / 64.0) * 0.5 + self.loss * 0.5;
        self.choke = (choked as f32 * 100.0 / 64.0) * 0.5 + self.choke * 0.5;
        let outgoing = stats.outgoing_sequence;
        for i in 0..64u32 {
            let seq = outgoing.wrapping_sub(63).wrapping_add(i);
            let c = &stats.commands[(seq & 63) as usize];
            let ring = (seq as usize) & (RING - 1);
            self.lerp[ring] = c.frame_lerp;
            self.size[ring] = c.sendsize;
            self.sent[ring] = !c.heldback;
        }

        if level < 3 {
            let mut cur_h = 0;
            for i in 0..w.max(0) {
                let x = gx + w - 1 - i;
                let seq = incoming.wrapping_sub(i as u32);
                let ring = (seq as usize) & (RING - 1);
                let st = self.state[ring];
                if st < 9997 {
                    cur_h = st;
                    let h = cur_h.min(hmax);
                    let color = if self.choked[ring] { [255, 255, 0, 255] } else { [63, 255, 63, 150] };
                    fill(screen, x, by - h, 1, 1, color);
                } else {
                    let h = cur_h.min(hmax);
                    let color = if st == DROPPED { [255, 0, 0, 255] } else if st == INVALID { [0, 0, 255, 255] } else { [240, 127, 63, 255] };
                    if h >= 4 {
                        fill(screen, x, by - h, 1, 2, color);
                        fill(screen, x, by - 2, 1, 2, color);
                    } else {
                        fill(screen, x, by - h, 1, h, color);
                    }
                }
                let edge = i <= 3 || i >= w - 4;
                if edge {
                    fill(screen, x, by, 1, 1, [0, 255, 0, 160]);
                }
                if level >= 2 {
                    let top = (by as f32 - graphheight - 1.0) as i32;
                    if edge {
                        fill(screen, x, top, 1, 1, [255, 255, 255, 255]);
                    }
                    if st <= 9995 {
                        self.stack(screen, x, top, &self.bytes[ring], scale, by as f32 - graphheight);
                    }
                }
            }
            if level >= 2 {
                // The byte ruler.
                let step = ((10.0 / scale) as i32).max(1);
                let y0 = (by as f32 - graphheight - 1.0) as i32;
                let mut y = y0;
                while y > 0 {
                    let bytes = (y0 - y) as f32 * scale;
                    if (f32::from(maxmsg) + 50.0) <= bytes {
                        break;
                    }
                    if (bytes as i32) % 50 == 0 {
                        fill(screen, gx, y, 3, 1, [0, 200, 0, 255]);
                    } else if step > 5 {
                        fill(screen, gx, y, 3, 1, [63, 63, 0, 200]);
                    }
                    y -= step;
                }
            }
            let updaterate = cvars.value("cl_updaterate") as i32;
            text(screen, (gx + w - 25).max(1), (by as f32 - graphheight).max(1.0) as i32, &format!("{updaterate}/s"));
            self.outgoing(screen, cvars, gx, w, sh, outgoing);
        }
        self.text_block(screen, cvars, stats, level, gx, (by as f32 - graphheight) as i32, frametime, ms_sum, ms_count);
    }

    /// The per-category byte pixels of one valid frame, stacked upwards from `top`.
    fn stack(&self, screen: &mut Screen, x: i32, top: i32, b: &NetBytes, scale: f32, base: f32) {
        let part = |v: u16| (f32::from(v) / scale) as i32;
        let mut y = top - part(b.client) - 1;
        let layers: [(u16, [i32; 4]); 8] = [
            (0, [255, 0, 0, 128]),
            (b.playerinfo, [255, 255, 0, 128]),
            (b.packetentities, [255, 0, 255, 128]),
            (b.tentity, [0, 0, 255, 128]),
            (b.sound, [0, 255, 0, 128]),
            (b.event, [0, 255, 255, 128]),
            (b.usr, [200, 200, 200, 128]),
            (b.voice, [255, 255, 255, 255]),
        ];
        for (k, (v, color)) in layers.into_iter().enumerate() {
            if k > 0 {
                y -= part(v);
            }
            if y < 2 {
                return;
            }
            fill(screen, x, y, 1, 1, color);
        }
        let yt = ((base - 1.0) - part(b.msgbytes) as f32) as i32;
        if yt >= 2 {
            fill(screen, x, yt, 1, 1, [240, 240, 240, 128]);
        }
    }

    /// The outgoing graph (`sub_101DE000`): `cl_cmdrate`, the lerp ramp, ticks and held-back
    /// commands.
    fn outgoing(&self, screen: &mut Screen, cvars: &Cvars, gx: i32, w: i32, sh: i32, outgoing: u32) {
        let cmdrate = cvars.value("cl_cmdrate") as i32;
        text(screen, (gx + w - 26).max(1), (sh - 27).max(1), &format!("{cmdrate}/s"));
        for i in 0..w.max(0) {
            let x = gx + w - 1 - i;
            let ring = (outgoing.wrapping_sub(i as u32) as usize) & (RING - 1);
            let v = (f64::from(self.lerp[ring]) / 3.0 * 24.0).min(24.0) as i32;
            if v < 8 {
                if let Some(c) = usize::try_from(v).ok().and_then(|i| RAMP.get(i)) {
                    fill(screen, x, sh - 4 - v, 1, 1, [c[0], c[1], c[2], 255]);
                }
            } else if let Some(c) = RAMP.get((v - 1) as usize) {
                fill(screen, x, sh - 4 - v + 1, 1, 1, [c[0], c[1], c[2], 255]);
            }
            if i <= 3 || i >= w - 4 {
                fill(screen, x, sh - 12, 1, 1, [255, 255, 255, 255]);
            }
            if !self.sent[ring] {
                fill(screen, x, sh - 3, 1, 1, [255, 0, 0, 200]);
            }
        }
    }

    /// `NetGraph_DrawTextFields`: fps and latency, in/out bytes and rates, loss and choke.
    #[allow(clippy::too_many_arguments)]
    fn text_block(&mut self, screen: &mut Screen, cvars: &Cvars, stats: &NetStats, level: i32, x: i32, y: i32, frametime: f64, ms_sum: f64, ms_count: i32) {
        self.fps_avg = (f64::from(self.fps_avg * 0.5) + frametime * 0.5) as f32;
        if self.fps_avg <= 0.0 {
            return;
        }
        text(screen, x, y, &format!("{:.1} fps", 1.0 / self.fps_avg));
        let updaterate = cvars.value("cl_updaterate");
        let mut ms = if ms_count == 0 { 0.0 } else { ((ms_sum / f64::from(ms_count)) as f32) - (frametime * 0.5) as f32 };
        if ms_count != 0 && updaterate > 0.0 {
            ms -= (1000.0 / updaterate) as i32 as f32;
        }
        let ms = ms.max(0.0);
        if ms > 1.0 {
            text(screen, x + 75, y, &format!("{} ms ", ms as i32));
        }
        let incoming = stats.incoming_sequence as usize & (RING - 1);
        text(screen, x, y + 15, &format!("in :  {} {:.2} k/s", self.bytes[incoming].msgbytes, stats.flow_kbytes[1]));
        let mut out = self.size[stats.outgoing_sequence.wrapping_sub(1) as usize & (RING - 1)];
        if out == 0 {
            out = self.last_out;
        } else {
            self.last_out = out;
        }
        text(screen, x, y + 30, &format!("out:  {out} {:.2} k/s", stats.flow_kbytes[0]));
        if level > 2 {
            text(screen, x, y + 45, &format!("loss: {} choke:  {}", (self.loss + 0.49) as i32, (self.choke + 0.49) as i32));
        }
    }
}

/// Text in (0.9, 0.9, 0.7), EngineFont; the colour then reverts to `con_color`.
fn text(screen: &mut Screen, x: i32, y: i32, s: &str) {
    screen.set_text_color(0.9, 0.9, 0.7);
    screen.draw_console_string(x, y, s);
}
