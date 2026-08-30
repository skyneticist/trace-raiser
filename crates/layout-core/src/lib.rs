//! Deterministic, legality-first board layout primitives.
//!
//! This crate owns the canonical optimization model. File-format adapters and
//! user interfaces sit outside this boundary so candidate solutions can be
//! replayed, evaluated, and compared without depending on KiCad serialization.

mod geometry;
mod jumpers;
mod model;
mod placer;
mod router;
mod routing;

pub use jumpers::{accept_jumper_proposal, propose_jumpers, JumperProposalError};
pub use model::*;
pub use placer::{place, validate_design, DesignValidationError};
pub use router::{route, RoutingError};
pub use routing::{validate_route_solution, RouteValidationError};
