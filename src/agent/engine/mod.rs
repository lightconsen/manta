//! Agent message-processing engine: turn loop, completions, tool dispatch,
//! and quality monitoring. Split by concern into `impl Agent` blocks across
//! submodules.

mod completion;
mod controller;
mod monitor;
mod tools;
mod turn;

#[cfg(test)]
mod tests;
