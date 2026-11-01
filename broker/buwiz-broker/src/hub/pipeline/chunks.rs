//! Runtime chunks (`emitChunk`, `chunkKey`): `nsh build` on one module into
//! `.stage/chunk-<name>`, its `bundle.js` copied to `chunks/<name>.js`.
//! Best-effort, as Node's: a failure logs nsh's last line and leaves the
//! card on its "module missing" path; a missing `chunks/` drops the copy.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::hub::traits::Nsh;

/// Chunk ids are flat `<stem>.js`: "R6@v3" ships as "R6v3".
pub fn chunk_key(key: &str) -> String {
    let Some((name, v)) = key.split_once("@v") else { return key.to_owned() };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    match name.strip_prefix('R') {
        Some(n) if digits(n) && digits(v) => format!("R{n}v{v}"),
        _ => key.to_owned(),
    }
}

/// `emitChunk(name, srcFile)`.
pub async fn emit_chunk(nsh: Arc<dyn Nsh>, stage: PathBuf, chunks: PathBuf, name: String, src: PathBuf) {
    let out = stage.join(format!("chunk-{name}"));
    let _ = std::fs::remove_dir_all(&out);
    match nsh.build(&src, &out).await {
        Err(e) => eprintln!("chunk {name}: {}", last_line(&e)),
        Ok(()) => {
            let _ = std::fs::copy(out.join("bundle.js"), chunks.join(format!("{name}.js")));
        }
    }
}

/// `String(err).trim().split("\n").pop()`
fn last_line(e: &str) -> &str {
    e.trim_matches(crate::js::is_space).rsplit('\n').next().unwrap_or("")
}

/// The chunk builds a write started (Node fires them and forgets): dropped,
/// they run on; `done` waits for them.
#[derive(Default)]
pub struct ChunkJobs(pub Vec<tokio::task::JoinHandle<()>>);

impl ChunkJobs {
    pub fn spawn(&mut self, nsh: &Arc<dyn Nsh>, stage: &Path, chunks: &Path, name: String, src: PathBuf) {
        self.0.push(tokio::spawn(emit_chunk(nsh.clone(), stage.to_owned(), chunks.to_owned(), name, src)));
    }

    pub async fn done(self) {
        for j in self.0 {
            let _ = j.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipe_07_chunk_keys() {
        assert_eq!(chunk_key("R6@v3"), "R6v3");
        assert_eq!(chunk_key("R6"), "R6");
        assert_eq!(chunk_key("x@v3"), "x@v3");
        assert_eq!(chunk_key("R6@v"), "R6@v");
        assert_eq!(chunk_key("R@v1"), "R@v1");
        assert_eq!(last_line("first\nlast line\n"), "last line");
    }
}
