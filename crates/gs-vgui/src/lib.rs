//! vgui2 and vgui_controls as GameUI uses them: the panel tree with its popups, z-order and
//! clip rects (docs/re/vgui2-core.md §1–§3), the message queues, keyboard and mouse focus, scheme
//! colours/fonts/borders (§4) and the controls (docs/re/vgui-controls.md,
//! docs/re/vgui-controls-2.md). Drawing goes through [`Surface`], which the engine implements
//! (docs/re/vgui-surface.md).

pub mod border;
pub mod controls;
pub mod editable;
pub mod keys;
pub mod keyvalues;
pub mod msg;
pub mod scheme;
pub mod surface;
mod tooltip;
mod ui;
mod user_config;

pub use tooltip::Tooltip;

pub use msg::{Msg, Value};
pub use surface::{Color, Cursor, FontHandle, Surface};
pub use ui::{Control, Node, PanelId, PlainPanel, Ui};

#[cfg(test)]
mod tests;