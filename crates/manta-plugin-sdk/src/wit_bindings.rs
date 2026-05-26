//! WIT Component Model bindings
//!
//! Auto-generated host function stubs from Manta's WIT definitions.
//! Requires the `wit` feature flag.
//!
//! # Usage
//!
//! ```ignore
//! use manta_plugin_sdk::wit_bindings::*;
//!
//! let id = host_functions::get_plugin_id();
//! host_functions::log("info", &format!("My ID: {}", id));
//! let config = host_functions::config_get_all();
//! ```

wit_bindgen::generate!("host-bindings" in "../../wit/plugin-sdk");

pub use self::manta::plugin_sdk::host_functions;
