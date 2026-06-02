//! quokka-core — the presentation-free heart of quokka.
//!
//! This crate is everything the CLI and the (future) Tauri GUI share: the
//! [`device`] seam (the `Device` trait + the iOS/Android/fake backends and
//! their neutral output types), the application [`app`] facade that drives the
//! device and returns serializable DTOs, and the pure logic that projects and
//! formats those values ([`fmt`], [`logic`], [`card`]).
//!
//! Nothing here knows about terminals, ratatui, clap, or Tauri. Surfaces
//! consume this crate: the CLI renders the DTOs as text and re-exports these
//! modules at its own paths; the GUI wraps each [`app`] function in a one-line
//! command. No `idevice` / `forensic-adb` type leaks through the public
//! surface of [`device`] — that seam absorbs their pre-1.0 churn.

pub mod app;
pub mod card;
pub mod device;
pub mod fmt;
pub mod logic;
