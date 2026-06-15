//! Adapters that bridge existing subsystem types to the [`Capability`](super::Capability) trait.
//!
//! These providers wrap types from `src/tools/` and `src/computer/` so they
//! can be registered in a [`CapabilityRegistry`](super::registry::CapabilityRegistry)
//! and invoked uniformly through the capability interface.

pub mod tool_adapter;
pub mod computer_adapter;
pub mod device_adapter;
