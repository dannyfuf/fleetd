//! Shared GUI-driving protocol, input dispatch, and transport seams.
//!
//! Two features split what a consumer links. `legacy` is the lazygit driver's file-script
//! transport; `socket` is the harness command socket — the protocol, the server loop, input
//! dispatch and the predicate evaluator — and implies `legacy`, because `input` shares its
//! keystroke translation. `fleet-lazygit` takes `legacy` alone, so the predicate evaluator and
//! the server loop stay out of a binary that drives nothing over a socket.

#[cfg(feature = "socket")]
pub mod input;
#[cfg(feature = "legacy")]
pub mod legacy;
#[cfg(feature = "socket")]
pub mod predicate;
#[cfg(feature = "socket")]
pub mod protocol;
#[cfg(feature = "socket")]
pub mod server;
