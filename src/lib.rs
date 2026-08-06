//! Carrying a light-wallet gRPC connection over a mixnet, transparently to both ends.
//!
//! Two processes, each a byte pipe:
//!
//! ```text
//! wallet --TCP--> [lwd-mixnet-client] --mixnet--> [lwd-mixnet-server] --TCP--> lightwalletd
//! ```
