pub mod analyze;
pub mod apps;
pub mod capture;
pub mod card;
pub mod dashboard;
pub mod device_action;
pub mod devices;
pub mod info;
pub mod logs;
pub mod media;
pub mod menu;
pub mod power;
pub mod sidebar;
pub mod status;
pub mod update;

// `top_n_by_size` is pure ranking logic that now lives in the core
// (`crate::logic`). Re-export it at the historical `commands::top_n_by_size`
// path so call sites (`super::top_n_by_size`) and tests keep resolving.
pub use crate::logic::top_n_by_size;
