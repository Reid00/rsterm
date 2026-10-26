// src/ssh/mod.rs
//! SSH-related utilities
//!
//! This module provides SSH functionality including known hosts management.

mod config;
mod known_hosts;

pub use config::{SshConfigEntry, SshConfigId, SshConfigSnapshot, SshConfigSource};
pub use known_hosts::*;
