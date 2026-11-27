//! Core substrate for the Super compiler: arena IDs, interner, spans, diagnostics.

#![deny(unsafe_op_in_unsafe_fn)]

mod arena;
mod diagnostic;
mod ids;
mod interner;
mod span;
pub mod traits;

pub use arena::{Arena, NodeStore};
pub use diagnostic::{Diagnostic, DiagnosticKind, Diagnostics, Severity, err, wants_color};
pub use ids::{NodeId, SymId, TypeId};
pub use interner::Interner;
pub use span::{BytePos, SourceFile, Span};
