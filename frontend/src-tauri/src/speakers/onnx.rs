//! Small helpers around ONNX Runtime sessions for the speaker models.

use std::path::Path;

use anyhow::{anyhow, Result};
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;

pub(crate) fn open(path: &Path) -> Result<Session> {
    open_with_threads(path, std::thread::available_parallelism().map(|n| n.get().min(4)).unwrap_or(2))
}

/// [`open`] on a given number of threads (background work takes few)
pub(crate) fn open_with_threads(path: &Path, threads: usize) -> Result<Session> {
    Session::builder()?
        .with_optimization_level(GraphOptimizationLevel::Level3)?
        .with_intra_threads(threads)?
        .commit_from_file(path)
        .map_err(|e| anyhow!("Could not open {}: {e}", path.display()))
}

/// Custom metadata value of a model, e.g. "window_size"
pub(crate) fn meta(session: &Session, key: &str) -> Option<String> {
    session.metadata().ok()?.custom(key).ok().flatten()
}

/// Inputs, outputs and custom metadata, for logs and checks
pub(crate) fn describe(session: &Session) -> String {
    let mut out = String::new();
    for i in &session.inputs {
        out.push_str(&format!("input {} {:?}\n", i.name, i.input_type));
    }
    for o in &session.outputs {
        out.push_str(&format!("output {} {:?}\n", o.name, o.output_type));
    }
    if let Ok(m) = session.metadata() {
        for k in m.custom_keys().unwrap_or_default() {
            out.push_str(&format!("meta {k} = {:?}\n", m.custom(&k).ok().flatten()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore]
    fn describe_models() {
        for p in std::env::var("PROTOKOL_SPEAKER_MODELS").unwrap().split(':') {
            let s = super::open(std::path::Path::new(p)).unwrap();
            println!("== {p}\n{}", super::describe(&s));
        }
    }
}
