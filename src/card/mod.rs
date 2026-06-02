//! Pure share-card layers: the `CardData` projection, badge evaluation, the
//! SVG renderer, the PNG rasterizer, the emoji glyphs, and the share-URL
//! builder. Every layer is a pure function of its input — no device, no
//! terminal, no filesystem — so the same device state always renders to
//! byte-identical output.
//!
//! The `quokka card` command's `run` (which writes the PNG and opens Preview)
//! lives in `crate::commands::card` and consumes these. The facade
//! (`crate::app::card`) and the GUI consume them directly.

pub mod badges;
pub mod data;
pub mod emoji;
pub mod png;
pub mod render;
pub mod share;
