//! Native GPUI terminal panes. Alacritty owns ANSI state; portable-pty owns the OS PTY.
mod appearance;
mod engine;
mod input;
mod view;
pub use appearance::TerminalAppearance;
pub use engine::{TerminalSession, TerminalSpec};
pub use input::encode_key;
pub use view::TerminalView;
