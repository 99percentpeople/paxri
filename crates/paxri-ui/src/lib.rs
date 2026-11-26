//! GPUI controls and terminal rendering shared by the desktop and Android clients.
pub mod components;
pub mod connections;
pub mod settings;
pub mod terminal;
pub mod theme;

#[cfg(windows)]
pub mod ime;
