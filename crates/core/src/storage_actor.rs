use std::path::Path;
use std::sync::mpsc;
use std::thread;

use storage::{Storage, StorageError};
use tokio::sync::oneshot;

type Job = Box<dyn FnOnce(&Storage) + Send>;

/// アプリ内 SQLite を専用スレッドで動かす。同期 API の rusqlite で、Tokio のワーカーや
/// UI スレッドを塞がないため。仕事は送った順に実行される。
#[derive(Clone)]
pub(crate) struct StorageHandle {
    tx: mpsc::Sender<Job>,
}

impl StorageHandle {
    pub fn spawn(path: Option<&Path>) -> Result<Self, StorageError> {
        let storage = match path {
            Some(p) => Storage::open(p)?,
            None => Storage::open_in_memory()?,
        };
        let (tx, rx) = mpsc::channel::<Job>();
        thread::Builder::new()
            .name("storage".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    job(&storage);
                }
            })
            .map_err(StorageError::Io)?;
        Ok(Self { tx })
    }

    /// 仕事を送る。結果は待たなくてよい(受け取る側を捨てても、仕事は実行される)。
    pub fn submit<R: Send + 'static>(
        &self,
        f: impl FnOnce(&Storage) -> Result<R, StorageError> + Send + 'static,
    ) -> oneshot::Receiver<Result<R, StorageError>> {
        let (reply, rx) = oneshot::channel();
        let job: Job = Box::new(move |s| {
            let _ = reply.send(f(s));
        });
        // スレッドは Storage と一緒に生き続けるため、送信に失敗するのは終了処理中だけ
        let _ = self.tx.send(job);
        rx
    }

    pub async fn call<R: Send + 'static>(
        &self,
        f: impl FnOnce(&Storage) -> Result<R, StorageError> + Send + 'static,
    ) -> Result<R, StorageError> {
        self.submit(f)
            .await
            .unwrap_or_else(|_| Err(StorageError::InvalidData("storage thread stopped".into())))
    }
}
