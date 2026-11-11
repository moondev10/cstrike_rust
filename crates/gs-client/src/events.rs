//! The engine event queue (`cl_events`, docs/re/client-sim.md Part E §1.2, §2.3–§2.7): events
//! from the server (and later from client prediction) wait here until `CL_FireEvents` hands
//! them to the client DLL's hooks.

use gs_protocol::structs::EventArgs;

/// `FEV_RELIABLE`.
pub const FEV_RELIABLE: i32 = 2;
const MAX_EVENTS: usize = 64;

#[derive(Debug, Clone, Default)]
struct Slot {
    /// Event precache index; 0 = free.
    index: u16,
    /// Absolute `cl.time`, or 0 to fire on the next pass.
    fire_time: f32,
    args: EventArgs,
    flags: i32,
}

/// An event due for its hook.
#[derive(Debug, Clone, PartialEq)]
pub struct FiredEvent {
    /// The queue slot (`cl_showevents` prints it and uses `slot & 31` as the notify line).
    pub slot: usize,
    pub index: u16,
    /// The event script name (`events/ak47.sc`), from the precache table.
    pub name: Option<String>,
    pub args: EventArgs,
}

#[derive(Debug, Clone)]
pub struct EventQueue {
    slots: Vec<Slot>,
}

impl Default for EventQueue {
    fn default() -> Self {
        Self { slots: vec![Slot::default(); MAX_EVENTS] }
    }
}

impl EventQueue {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// `CL_QueueEvent`: the first free slot; a reliable event may evict an unreliable one when
    /// the queue is full, anything else is dropped.
    pub fn queue(&mut self, flags: i32, index: u16, delay: f32, args: &EventArgs, time: f64) {
        let slot = self.slots.iter().position(|s| s.index == 0).or_else(|| {
            (flags & FEV_RELIABLE != 0).then(|| self.slots.iter().position(|s| s.flags & FEV_RELIABLE == 0)).flatten()
        });
        let Some(i) = slot else {
            log::debug!("event {index} dropped: queue full");
            return;
        };
        let fire_time = if delay == 0.0 { 0.0 } else { (time + f64::from(delay)) as f32 };
        self.slots[i] = Slot { index, fire_time, args: args.clone(), flags };
    }

    /// `CL_FireEvents`: every due event in slot order (not time order); the slots are freed.
    pub fn fire(&mut self, time: f64, names: &[Option<String>]) -> Vec<FiredEvent> {
        let mut out = Vec::new();
        for (slot, s) in self.slots.iter_mut().enumerate() {
            if s.index == 0 || (s.fire_time != 0.0 && f64::from(s.fire_time) > time) {
                continue;
            }
            let name = names.get(usize::from(s.index)).cloned().flatten();
            out.push(FiredEvent { slot, index: s.index, name, args: std::mem::take(&mut s.args) });
            *s = Slot::default();
        }
        out
    }
}

/// `COM_NormalizeAngles`: components beyond ±180 wrap through `fmod` (370 becomes −350, as in
/// the original).
pub fn com_normalize_angles(a: [f32; 3]) -> [f32; 3] {
    a.map(|v| {
        if v > 180.0 {
            v % 360.0 - 360.0
        } else if v < -180.0 {
            v % 360.0 + 360.0
        } else {
            v
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(entindex: i32) -> EventArgs {
        let mut a = EventArgs::default();
        a.set_entindex(entindex);
        a
    }

    #[test]
    fn delayed_events_wait_and_fire_in_slot_order() {
        let mut q = EventQueue::default();
        q.queue(0, 5, 0.5, &args(1), 10.0);
        q.queue(0, 6, 0.0, &args(2), 10.0);
        let names = vec![None; 8];
        let now: Vec<u16> = q.fire(10.0, &names).iter().map(|e| e.index).collect();
        assert_eq!(now, vec![6]);
        let later: Vec<u16> = q.fire(10.5, &names).iter().map(|e| e.index).collect();
        assert_eq!(later, vec![5]);
        assert!(q.fire(11.0, &names).is_empty());
    }

    #[test]
    fn full_queue_drops_unreliable_and_reliable_evicts() {
        let mut q = EventQueue::default();
        for i in 0..MAX_EVENTS {
            q.queue(0, 1, 1.0, &args(i as i32), 0.0);
        }
        q.queue(0, 2, 0.0, &args(99), 0.0);
        q.queue(FEV_RELIABLE, 3, 0.0, &args(100), 0.0);
        let fired = q.fire(0.0, &[]);
        assert_eq!(fired.iter().map(|e| e.index).collect::<Vec<_>>(), vec![3], "only the reliable one is due");
        assert_eq!(fired[0].args.entindex(), 100);
    }

    #[test]
    fn normalize_matches_the_engine_quirk() {
        assert_eq!(com_normalize_angles([370.0, -190.0, 90.0]), [-350.0, 170.0, 90.0]);
    }
}
