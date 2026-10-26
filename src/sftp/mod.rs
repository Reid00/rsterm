// src/sftp/mod.rs
//! SFTP client module for file transfers over SSH
//!
//! This module provides SFTP (SSH File Transfer Protocol) capabilities
//! for transferring files to and from remote servers.

mod client;

#[cfg(test)]
pub(crate) use client::MockSftpSessionOps;

pub use client::{
    remote_parent, FileEntry, FileMetadata, SftpClient, SftpFileReadOps, SftpFileWriteOps,
    SftpSessionOps,
};
