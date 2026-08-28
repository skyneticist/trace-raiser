//! Deterministic, legality-first board layout primitives.
//!
//! This crate owns the canonical optimization model. File-format adapters and
//! user interfaces sit outside this boundary so candidate solutions can be
//! replayed, evaluated, and compared without depending on KiCad serialization.

mod geometry;
mod model;
mod placer;
mod routing;

pub use model::*;
pub use placer::{place, validate_design, DesignValidationError};
pub use routing::{validate_route_solution, RouteValidationError};
