//! Background writer for window state.
//!
//! Atomic saves sync the file and its directory, and on a contended filesystem
//! each sync can block for many seconds. Window state is saved from the GTK
//! thread on a timer and after user actions, so those syncs run here instead:
//! the GTK thread only builds a snapshot and queues it.
//!
//! One thread serves the whole process, so saves from several windows are
//! written in submission order. Snapshots that queue up behind a slow write
//! are coalesced: only the newest one per store is written.

use super::{ClientStore, WindowStateDocuments};
use std::sync::OnceLock;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};

enum Job {
    Save(ClientStore, Box<WindowStateDocuments>),
    Flush(SyncSender<()>),
}

fn sender() -> Option<&'static Sender<Job>> {
    static SENDER: OnceLock<Option<Sender<Job>>> = OnceLock::new();
    SENDER
        .get_or_init(|| {
            let (tx, rx) = mpsc::channel();
            match std::thread::Builder::new()
                .name("rttx-state-writer".into())
                .spawn(move || run(&rx))
            {
                Ok(_) => Some(tx),
                Err(e) => {
                    tracing::error!("Failed to start state writer thread: {e}");
                    None
                }
            }
        })
        .as_ref()
}

/// Queue a window state snapshot to be written in the background.
///
/// Falls back to writing on the calling thread if the writer is unavailable.
pub fn submit(store: ClientStore, documents: WindowStateDocuments) {
    let job = Job::Save(store, Box::new(documents));
    let unsent = match sender() {
        Some(tx) => tx.send(job).err().map(|e| e.0),
        None => Some(job),
    };
    if let Some(Job::Save(store, documents)) = unsent {
        store.save_window_state(&documents);
    }
}

/// Block until every snapshot submitted so far has been written.
pub fn flush() {
    let Some(tx) = sender() else {
        return;
    };
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    if tx.send(Job::Flush(done_tx)).is_ok() {
        let _ = done_rx.recv();
    }
}

fn run(rx: &Receiver<Job>) {
    while let Ok(first) = rx.recv() {
        let mut pending: Vec<(ClientStore, Box<WindowStateDocuments>)> = Vec::new();
        let mut waiters = Vec::new();
        for job in std::iter::once(first).chain(rx.try_iter()) {
            match job {
                Job::Save(store, documents) => {
                    pending.retain(|(queued, _)| *queued != store);
                    pending.push((store, documents));
                }
                Job::Flush(done) => waiters.push(done),
            }
        }
        for (store, documents) in &pending {
            store.save_window_state(documents);
        }
        for done in waiters {
            let _ = done.send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{StorePaths, models};
    use tempfile::TempDir;

    fn test_store(tmp: &TempDir) -> ClientStore {
        ClientStore::new(StorePaths::new(
            tmp.path().join("config"),
            tmp.path().join("state"),
            tmp.path().join("cache"),
        ))
    }

    fn documents(width: i32) -> WindowStateDocuments {
        WindowStateDocuments {
            workspaces: models::workspaces::WorkspaceStore::default(),
            ui: models::ui::UiState { window_width: width, ..Default::default() },
            runtime_cache: models::runtime_cache::RuntimeCache::default(),
        }
    }

    #[test]
    fn flush_waits_for_submitted_state() {
        let tmp = TempDir::new().unwrap();
        let store = test_store(&tmp);
        submit(store.clone(), documents(1234));
        flush();
        assert_eq!(store.load_ui_state().into_value().unwrap().window_width, 1234);
        assert!(store.paths().state().join("workspaces.json").exists());
        assert!(store.paths().cache().join("runtime-cache.json").exists());
    }

    #[test]
    fn newest_submission_wins() {
        let tmp = TempDir::new().unwrap();
        let store = test_store(&tmp);
        for width in 1000..1020 {
            submit(store.clone(), documents(width));
        }
        flush();
        assert_eq!(store.load_ui_state().into_value().unwrap().window_width, 1019);
    }

    #[test]
    fn unchanged_state_is_not_rewritten() {
        let tmp = TempDir::new().unwrap();
        let store = test_store(&tmp);
        submit(store.clone(), documents(800));
        flush();
        let path = store.paths().state().join("ui.json");
        let before = std::fs::read_to_string(&path).unwrap();

        submit(store, documents(800));
        flush();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        assert!(!path.with_extension("bak").exists());
    }
}
