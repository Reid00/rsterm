// src/ssh/mod.rs
//! SSH-related utilities
//!
//! This module provides SSH functionality including known hosts management.

mod known_hosts;

pub use known_hosts::*;