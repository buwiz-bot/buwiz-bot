//! The jobs table (Node's `jobs` Map in state.mjs: what `/status` reads
//! while a chat turn runs). Empty in the skeleton: track A (5.1, 5.2) fills
//! it with the job record, its event ring and the 25-job cap.

/// A job's id (Node's `nextJobId`, counting from 1).
pub type JobId = u64;

#[derive(Default)]
pub struct Jobs {}
