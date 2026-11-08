//! Test support: an in-memory backend with scripted faults, a crash/fault
//! backend with a volatile/durable model, and one contract suite that every
//! backend must pass.

pub mod contract;
pub mod fault;
pub mod memory;
pub mod rng;

#[cfg(test)]
mod fault_tests;
