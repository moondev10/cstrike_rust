//! The HLTV codebase the engine runs for `viewdemo`: Core.dll's `World` and `Server` (the demo
//! ingestion and the HLTV-style stream writers, docs/re/hltv-world.md) and `DemoPlayer.dll`
//! (docs/re/demoplayer.md), hosted the way hw.dll's `SystemWrapper` hosts them
//! (docs/re/demoplayer-engine.md §4).

pub mod bitbuf;
pub mod delta;
pub mod dict;
pub mod director;
pub mod player;
pub mod server;
pub mod world;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// One console line from a module (`IBaseSystem::Printf` / `DPrintf`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogLine {
    /// `Con_Printf("%s", text)`.
    Print(String),
    /// `Con_DPrintf(text)` (developer only).
    DPrint(String),
}

/// The files the modules open (`IBaseSystem::GetFileSystem`).
pub trait Files {
    /// `Open(name, "rb")` + Read.
    fn read(&self, name: &str) -> Option<Vec<u8>>;
    /// `Open(name, "wb")` + Write.
    fn write(&self, name: &str, data: &[u8]) -> bool;
}

/// What the modules share of `SystemWrapper`: the console output, the stop flag set by `Errorf`,
/// the module clock and the file system.
pub struct SysState {
    pub log: RefCell<Vec<LogLine>>,
    /// `Stop()`: after an `Errorf` no module runs again.
    pub stopped: Cell<bool>,
    /// `GetTime()`: the accumulated host frame time.
    pub time: Cell<f64>,
    pub files: Box<dyn Files>,
}

/// The shared `IBaseSystem`.
#[derive(Clone)]
pub struct Sys(Rc<SysState>);

impl std::ops::Deref for Sys {
    type Target = SysState;
    fn deref(&self) -> &SysState {
        &self.0
    }
}

impl Sys {
    pub fn new(files: Box<dyn Files>) -> Self {
        Self(Rc::new(SysState { log: RefCell::new(Vec::new()), stopped: Cell::new(false), time: Cell::new(0.0), files }))
    }

    /// `Printf`.
    pub fn print(&self, s: &str) {
        self.log.borrow_mut().push(LogLine::Print(s.to_owned()));
    }

    /// `DPrintf`.
    pub fn dprint(&self, s: &str) {
        self.log.borrow_mut().push(LogLine::DPrint(s.to_owned()));
    }

    /// `Errorf`: the fatal banner, then `Stop()`.
    pub fn errorf(&self, s: &str) {
        self.print("***** FATAL ERROR *****\n");
        self.print(s);
        self.print("*** STOPPING SYSTEM ***\n");
        self.stopped.set(true);
    }

    /// The lines printed since the last call.
    pub fn take_log(&self) -> Vec<LogLine> {
        std::mem::take(&mut *self.log.borrow_mut())
    }
}
