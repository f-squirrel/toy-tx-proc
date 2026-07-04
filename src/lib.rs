//! Reusable payments engine library.
//!
//! The binary crate owns CLI concerns; this library owns the transaction model,
//! CSV boundary, and state machine so the same pipeline can be reused with any
//! streaming input source.

pub mod engine;
pub mod io;
pub mod model;
