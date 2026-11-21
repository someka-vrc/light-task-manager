//! The application shell: the Win32 window, the state behind it, and input helpers.

pub mod aim;
pub mod core;
pub mod instance;
pub mod pointer;
pub mod window;

pub use window::run;
