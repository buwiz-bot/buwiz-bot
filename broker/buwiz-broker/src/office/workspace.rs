//! `FileWorkspace` (agents/workspace.ts): the open documents, the files
//! under the root, and exports and creates that stay inside it and never
//! overwrite. File-system errors read as Node's (`ENOENT: no such file or
//! directory, realpath '…'`), since the tools pass them on.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

use serde_json::{json, Map, Value};

use super::docx::Docx;
use super::pptx::Pptx;
use super::proto::{Res, ToolError};
use super::xlsx::Xlsx;

pub enum Doc {
    Docx(Box<Docx>),
    Xlsx(Box<Xlsx>),
    Pptx(Box<Pptx>),
}

impl Doc {
    pub fn format(&self) -> &'static str {
        match self {
            Doc::Docx(_) => "docx",
            Doc::Xlsx(_) => "xlsx",
            Doc::Pptx(_) => "pptx",
        }
    }

    pub fn overview(&mut self) -> Res<Value> {
        match self {
            Doc::Docx(d) => d.overview(),
            Doc::Xlsx(d) => d.overview(),
            Doc::Pptx(d) => d.overview(),
        }
    }

    pub fn export(&mut self, proposal: Option<&str>) -> Res<Vec<u8>> {
        match self {
            Doc::Docx(d) => d.export(proposal),
            Doc::Xlsx(d) => d.export(proposal),
            Doc::Pptx(d) => d.export(proposal),
        }
    }

    pub fn close(&mut self) {
        match self {
            Doc::Docx(d) => d.close(),
            Doc::Xlsx(d) => d.close(),
            Doc::Pptx(d) => d.close(),
        }
    }
}

/// A Node `fs` error: its code and `uv`'s message.
pub fn fs_error(e: &std::io::Error, syscall: &str, path: &Path) -> ToolError {
    let (code, text) = match e.raw_os_error() {
        Some(libc::ENOENT) => ("ENOENT", "no such file or directory"),
        Some(libc::EEXIST) => ("EEXIST", "file already exists"),
        Some(libc::ENOTDIR) => ("ENOTDIR", "not a directory"),
        Some(libc::EACCES) => ("EACCES", "permission denied"),
        Some(libc::EPERM) => ("EPERM", "operation not permitted"),
        Some(libc::EISDIR) => ("EISDIR", "illegal operation on a directory"),
        Some(libc::ELOOP) => ("ELOOP", "too many symbolic links encountered"),
        Some(libc::ENAMETOOLONG) => ("ENAMETOOLONG", "name too long"),
        _ => return ToolError::new("ENGINE_ERROR", e.to_string()),
    };
    ToolError::system(code, format!("{code}: {text}, {syscall} '{}'", path.display()))
}

/// `path.resolve(base, p)`: lexical, `..` and `.` folded.
pub fn resolve(base: &Path, p: &str) -> PathBuf {
    let mut out = PathBuf::new();
    for c in base.join(p).components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// `path.relative(from, to)` for `to` inside `from`.
fn relative(from: &Path, to: &Path) -> String {
    to.strip_prefix(from).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| to.to_string_lossy().into_owned())
}

fn ext(p: &Path) -> String {
    p.extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_default()
}

/// `a.localeCompare(b)` for file names: case-insensitive first (spaces,
/// then punctuation, digits, letters), lowercase before uppercase on a tie.
pub fn locale_compare(a: &str, b: &str) -> std::cmp::Ordering {
    fn class(c: char) -> u8 {
        if c.is_whitespace() {
            0
        } else if c.is_alphabetic() {
            4
        } else if c.is_numeric() {
            3
        } else if c.is_ascii_punctuation() {
            1
        } else {
            2
        }
    }
    let key = |s: &str| -> Vec<(u8, String)> { s.chars().map(|c| (class(c), c.to_lowercase().collect())).collect() };
    key(a).cmp(&key(b)).then_with(|| {
        for (x, y) in a.chars().zip(b.chars()) {
            if x != y {
                return y.is_lowercase().cmp(&x.is_lowercase());
            }
        }
        std::cmp::Ordering::Equal
    })
}

pub struct Workspace {
    pub root: PathBuf,
    documents: Vec<(String, PathBuf, Doc)>,
    next_id: u64,
}

impl Workspace {
    /// `FileWorkspace.create(root)`.
    pub fn at(root: &Path) -> Res<Self> {
        let canonical = fs::canonicalize(root).map_err(|e| fs_error(&e, "realpath", root))?;
        if !canonical.is_dir() {
            return Err(ToolError::new("INVALID_ROOT", "Workspace root must be a directory."));
        }
        Ok(Self { root: canonical, documents: Vec::new(), next_id: 1 })
    }

    fn contained(&self, p: &Path) -> Res<PathBuf> {
        let absolute = resolve(&self.root, &p.to_string_lossy());
        if absolute == self.root || absolute.starts_with(&self.root) {
            return Ok(absolute);
        }
        Err(ToolError::new("OUTSIDE_WORKSPACE", "Choose a path inside the configured workspace root."))
    }

    /// `this.path(p)`: contained, real, and contained again.
    fn path(&self, p: &str) -> Res<PathBuf> {
        let lexical = self.contained(Path::new(p))?;
        let real = fs::canonicalize(&lexical).map_err(|e| fs_error(&e, "realpath", &lexical))?;
        self.contained(&real)
    }

    pub fn files(&self, directory: Option<&str>, offset: Option<f64>) -> Res<Value> {
        let offset = offset.unwrap_or(0.0) as usize;
        let folder = self.path(directory.unwrap_or("."))?;
        let mut entries: Vec<(String, bool)> = fs::read_dir(&folder)
            .map_err(|e| fs_error(&e, "scandir", &folder))?
            .filter_map(Result::ok)
            .filter_map(|e| {
                let t = e.file_type().ok()?;
                let name = e.file_name().to_string_lossy().into_owned();
                (t.is_dir() || t.is_file() && [".docx", ".xlsx", ".pptx"].contains(&ext(Path::new(&name)).as_str())).then_some((name, t.is_dir()))
            })
            .collect();
        entries.sort_by(|a, b| locale_compare(&a.0, &b.0));
        let files: Vec<Value> = entries
            .iter()
            .skip(offset)
            .take(100)
            .map(|(name, dir)| json!({ "path": relative(&self.root, &folder.join(name)), "kind": if *dir { "directory".to_owned() } else { ext(Path::new(name))[1..].to_owned() } }))
            .collect();
        let open: Vec<Value> = self.documents.iter().map(|(id, p, _)| json!({ "document": id, "path": relative(&self.root, p) })).collect();
        Ok(json!({ "files": files, "nextOffset": if offset + 100 < entries.len() { json!(offset + 100) } else { Value::Null }, "open": open }))
    }

    fn opened(id: &str, overview: Value) -> Value {
        let mut m = Map::new();
        m.insert("document".into(), json!(id));
        if let Value::Object(o) = overview {
            m.extend(o);
        }
        Value::Object(m)
    }

    pub fn open(&mut self, p: &str) -> Res<Value> {
        let file = self.path(p)?;
        let format = ext(&file);
        if ![".docx", ".xlsx", ".pptx"].contains(&format.as_str()) {
            return Err(ToolError::new("UNSUPPORTED_FORMAT", "Open a DOCX, XLSX, or PPTX file."));
        }
        if let Some((id, _, doc)) = self.documents.iter_mut().find(|(_, path, _)| *path == file) {
            let id = id.clone();
            return Ok(Self::opened(&id, doc.overview()?));
        }
        if self.documents.len() >= 10 {
            return Err(ToolError::new("DOCUMENT_LIMIT", "Close a document before opening more than 10."));
        }
        let mut handle = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(&file).map_err(|e| fs_error(&e, "open", &file))?;
        let info = self.validate(&file, &handle)?;
        if !info.is_file() || info.len() > 64 * 1024 * 1024 {
            return Err(ToolError::new("FILE_TOO_LARGE", "Open an Office file up to 64 MiB."));
        }
        let mut bytes = Vec::with_capacity(info.len() as usize + 1);
        std::io::Read::read_to_end(&mut handle, &mut bytes).map_err(|e| fs_error(&e, "read", &file))?;
        if bytes.len() as u64 != info.len() {
            return Err(ToolError::new("FILE_CHANGED", "The file changed during access. Retry with a stable workspace."));
        }
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut doc = match format.as_str() {
            ".xlsx" => Doc::Xlsx(Box::new(Xlsx::open(&bytes, &name)?)),
            ".pptx" => Doc::Pptx(Box::new(Pptx::open(&bytes, &name)?)),
            _ => Doc::Docx(Box::new(Docx::open(&bytes, &name)?)),
        };
        let id = format!("doc{}", self.next_id);
        self.next_id += 1;
        let overview = doc.overview()?;
        self.documents.push((id.clone(), file, doc));
        Ok(Self::opened(&id, overview))
    }

    pub fn get(&mut self, id: &str) -> Res<&mut Doc> {
        self.documents
            .iter_mut()
            .find(|(d, _, _)| d == id)
            .map(|(_, _, doc)| doc)
            .ok_or_else(|| ToolError::with("UNKNOWN_DOCUMENT", "Use office_open first, or office_files to list open document IDs.", json!({ "document": id })))
    }

    fn validate(&self, file: &Path, handle: &fs::File) -> Res<fs::Metadata> {
        let canonical = self.path(&file.to_string_lossy())?;
        let current = fs::symlink_metadata(&canonical).map_err(|e| fs_error(&e, "lstat", &canonical))?;
        let opened = handle.metadata().map_err(|e| fs_error(&e, "fstat", file))?;
        if current.file_type().is_symlink() || current.dev() != opened.dev() || current.ino() != opened.ino() {
            return Err(ToolError::new("FILE_CHANGED", "The file changed during access. Retry with a stable workspace."));
        }
        Ok(opened)
    }

    /// Writes `bytes` to a new file at `p` (export's and create's tail).
    fn write_new(&self, lexical: &Path, bytes: &[u8], code: &str, message: &str) -> Res<String> {
        let dir = lexical.parent().unwrap_or(&self.root).to_path_buf();
        let parent = fs::canonicalize(&dir).map_err(|e| fs_error(&e, "realpath", &dir))?;
        self.contained(&parent)?;
        let file = parent.join(lexical.file_name().unwrap_or_default());
        let mut handle = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&file).map_err(|e| fs_error(&e, "open", &file))?;
        let written = self.validate(&file, &handle).map(|_| ()).and_then(|_| handle.write_all(bytes).map_err(|e| fs_error(&e, "write", &file)));
        let rel = relative(&self.root, &file);
        if let Err(e) = written {
            return Err(ToolError::with(code, message, json!({ "path": rel, "cause": e.message })));
        }
        Ok(rel)
    }

    pub fn export(&mut self, id: &str, p: &str, proposal: Option<&str>) -> Res<Value> {
        let lexical = self.contained(Path::new(p))?;
        let doc = self.get(id)?;
        let format = doc.format();
        if ext(&lexical) != format!(".{format}") {
            return Err(ToolError::new("UNSUPPORTED_FORMAT", format!("Export path must end in .{format}.")));
        }
        let bytes = doc.export(proposal)?;
        let rel = self.write_new(&lexical, &bytes, "EXPORT_INCOMPLETE", "Export failed after creating its destination. Inspect the incomplete file and choose a new filename before retrying.")?;
        let mut out = json!({ "path": rel, "bytes": bytes.len() });
        if let Some(id) = proposal {
            out["proposal"] = json!(id);
            out["accepted"] = json!(false);
        }
        Ok(out)
    }

    pub fn create(&mut self, p: &str, bytes: &[u8]) -> Res<Value> {
        let lexical = self.contained(Path::new(p))?;
        if ![".docx", ".xlsx", ".pptx"].contains(&ext(&lexical).as_str()) {
            return Err(ToolError::new("UNSUPPORTED_FORMAT", "Create a .docx, .xlsx, or .pptx file."));
        }
        let rel = self.write_new(&lexical, bytes, "CREATE_INCOMPLETE", "Create failed after opening its destination. Inspect the incomplete file and choose a new filename before retrying.")?;
        Ok(json!({ "path": rel, "bytes": bytes.len() }))
    }

    pub fn close(&mut self, id: &str) -> Res<Value> {
        self.get(id)?.close();
        self.documents.retain(|(d, _, _)| d != id);
        Ok(json!({ "document": id, "closed": true }))
    }

    pub fn dispose(&mut self) {
        for (_, _, doc) in &mut self.documents {
            doc.close();
        }
        self.documents.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn office_files_sort_as_locale_compare_does() {
        let mut names = vec!["b.docx", "B.docx", "a-b.xlsx", "a.xlsx", "Zeta.pptx", "10.docx", "2.docx", "_x.docx"];
        names.sort_by(|a, b| locale_compare(a, b));
        assert_eq!(names, vec!["_x.docx", "10.docx", "2.docx", "a-b.xlsx", "a.xlsx", "b.docx", "B.docx", "Zeta.pptx"]);
    }

    #[test]
    fn office_paths_resolve_and_stay_inside() {
        let ws = Workspace { root: PathBuf::from("/r"), documents: Vec::new(), next_id: 1 };
        assert_eq!(ws.contained(Path::new("a/../b.docx")).unwrap(), PathBuf::from("/r/b.docx"));
        assert_eq!(ws.contained(Path::new("../x.docx")).unwrap_err().code, "OUTSIDE_WORKSPACE");
        assert_eq!(ws.contained(Path::new("/etc/x.docx")).unwrap_err().code, "OUTSIDE_WORKSPACE");
        assert_eq!(ws.contained(Path::new(".")).unwrap(), PathBuf::from("/r"));
    }
}
