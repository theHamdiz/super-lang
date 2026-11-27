//! Irreducible AOT host primitives emitted into C23 binaries.
//!
//! Higher-level networking (HTTP framing, routing, Conn/App) is Super
//! (`stdlib/net/`). Only OS ABI wrappers remain here.

mod net_host_c;

pub use net_host_c::NET_HOST_C;
