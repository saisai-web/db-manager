//! 結果ストアの内部: 状態、退避と読み戻し、専用の入出力スレッド。公開 API は `store`。

use std::io;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::thread;

use crate::chunk::Chunk;
use crate::spill::{Region, SpillFile};
use crate::types::{StoreConfig, StoreError};

pub(crate) type ReadyCallback = Arc<dyn Fn() + Send + Sync>;

pub(crate) struct Inner {
    pub(crate) ncols: usize,
    pub(crate) cfg: StoreConfig,
    pub(crate) state: Mutex<State>,
    pub(crate) on_ready: Mutex<Option<ReadyCallback>>,
}

pub(crate) struct State {
    /// 受信中のチャンク
    pub(crate) open: Chunk,
    /// 封印済みのチャンク。`first_row` の昇順
    pub(crate) slots: Vec<Slot>,
    pub(crate) sealed_rows: u64,
    /// 封印済みのうち、メモリにあるものの合計(書き出し待ちを含む)
    pub(crate) resident_bytes: usize,
    /// `resident_bytes` のうち、書き出し待ちで、完了すると解放されるもの
    pub(crate) pending_write_bytes: usize,
    pub(crate) spilled_bytes: u64,
    pub(crate) tick: u64,
    pub(crate) complete: bool,
    pub(crate) file: Option<Arc<SpillFile>>,
    pub(crate) io: Option<Sender<Job>>,
    pub(crate) failure: Option<(io::ErrorKind, String)>,
}

pub(crate) struct Slot {
    pub(crate) first_row: u64,
    pub(crate) mem: Option<Arc<Chunk>>,
    pub(crate) size: usize,
    pub(crate) file: Option<Region>,
    pub(crate) writing: bool,
    pub(crate) loading: bool,
    pub(crate) failed: bool,
    pub(crate) last_used: u64,
}

pub(crate) enum Job {
    Write { slot: usize, chunk: Arc<Chunk> },
    Read { slot: usize },
}

pub(crate) enum Loc {
    Open,
    Slot(usize),
    Missing,
}

impl State {
    pub(crate) fn locate(&self, row: u64) -> Loc {
        if row >= self.sealed_rows {
            return if row < self.sealed_rows + self.open.row_count as u64 {
                Loc::Open
            } else {
                Loc::Missing
            };
        }
        let i = self.slots.partition_point(|s| s.first_row <= row);
        Loc::Slot(i - 1)
    }
}

impl Inner {
    pub(crate) fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 受信中のチャンクを封印してメモリ上限を確認する。
    pub(crate) fn seal_open(self: &Arc<Self>, st: &mut State) {
        if st.open.is_empty() {
            return;
        }
        let next_first = st.open.first_row + st.open.row_count as u64;
        let mut chunk = std::mem::replace(&mut st.open, Chunk::new(next_first, self.ncols));
        chunk.shrink();
        let size = chunk.size_bytes();
        st.tick += 1;
        st.slots.push(Slot {
            first_row: chunk.first_row,
            mem: Some(Arc::new(chunk)),
            size,
            file: None,
            writing: false,
            loading: false,
            failed: false,
            last_used: st.tick,
        });
        st.sealed_rows = next_first;
        st.resident_bytes += size;
        self.enforce_limit(st);
    }

    /// 最も長く参照されていないチャンクから退避し、メモリ上限に収める。
    /// 直近に使ったチャンクは必ず残す(上限がチャンクより小さくても読めるように)。
    pub(crate) fn enforce_limit(self: &Arc<Self>, st: &mut State) {
        if self.cfg.spill_dir.is_none() {
            return;
        }
        loop {
            let effective = st.resident_bytes - st.pending_write_bytes;
            if effective <= self.cfg.memory_limit {
                return;
            }
            let mut candidates = 0;
            let mut victim: Option<usize> = None;
            for (i, s) in st.slots.iter().enumerate() {
                if s.mem.is_some() && !s.writing {
                    candidates += 1;
                    if victim.is_none_or(|v| s.last_used < st.slots[v].last_used) {
                        victim = Some(i);
                    }
                }
            }
            let Some(v) = victim.filter(|_| candidates > 1) else {
                return;
            };
            if st.slots[v].file.is_some() {
                // 一度書き出したチャンクは不変なので、メモリから外すだけでよい
                let slot = &mut st.slots[v];
                slot.mem = None;
                st.resident_bytes -= slot.size;
            } else {
                let chunk = st.slots[v].mem.clone().expect("candidate has memory");
                st.slots[v].writing = true;
                st.pending_write_bytes += st.slots[v].size;
                if !self.send_job(st, Job::Write { slot: v, chunk }) {
                    st.slots[v].writing = false;
                    st.pending_write_bytes -= st.slots[v].size;
                    return;
                }
            }
        }
    }

    /// 専用スレッド(初回に起動)へ仕事を渡す。起動できなければ false。
    pub(crate) fn send_job(self: &Arc<Self>, st: &mut State, job: Job) -> bool {
        if st.io.is_none() {
            let Some(dir) = self.cfg.spill_dir.as_ref() else {
                return false;
            };
            if st.file.is_none() {
                match SpillFile::create(dir) {
                    Ok(f) => st.file = Some(Arc::new(f)),
                    Err(e) => {
                        st.failure = Some((e.kind(), e.to_string()));
                        return false;
                    }
                }
            }
            let (tx, rx) = mpsc::channel();
            let weak = Arc::downgrade(self);
            let spawned = thread::Builder::new()
                .name("result-store-io".into())
                .spawn(move || io_loop(weak, rx));
            if let Err(e) = spawned {
                st.failure = Some((e.kind(), e.to_string()));
                return false;
            }
            st.io = Some(tx);
        }
        st.io.as_ref().is_some_and(|tx| tx.send(job).is_ok())
    }

    pub(crate) fn complete_write(self: &Arc<Self>, slot: usize, chunk: &Chunk) {
        let file = self.lock().file.clone().expect("spill file exists");
        let result = file.append(&chunk.encode());
        let mut st = self.lock();
        let size = st.slots[slot].size;
        st.pending_write_bytes -= size;
        st.slots[slot].writing = false;
        match result {
            Ok(region) => {
                st.slots[slot].file = Some(region);
                st.slots[slot].mem = None;
                st.resident_bytes -= size;
                st.spilled_bytes += region.len;
            }
            // 書き出せなくてもデータは失わない。以降の追記を止めて呼び出し側に知らせる
            Err(e) => st.failure = Some((e.kind(), e.to_string())),
        }
    }

    pub(crate) fn complete_read(self: &Arc<Self>, slot: usize) {
        let (file, region) = {
            let st = self.lock();
            (st.file.clone(), st.slots[slot].file)
        };
        let loaded = match (file, region) {
            (Some(file), Some(region)) => file
                .read(region)
                .and_then(|bytes| Chunk::decode(&bytes))
                .map(Arc::new),
            _ => Err(io::Error::other("chunk is not on disk")),
        };
        {
            let mut st = self.lock();
            st.slots[slot].loading = false;
            match loaded {
                Ok(chunk) => self.install(&mut st, slot, chunk),
                Err(_) => st.slots[slot].failed = true,
            }
        }
        let callback = self
            .on_ready
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(cb) = callback {
            cb();
        }
    }

    /// 読み戻したチャンクをメモリに戻す。すでに戻っていれば何もしない。
    pub(crate) fn install(self: &Arc<Self>, st: &mut State, slot: usize, chunk: Arc<Chunk>) {
        if st.slots[slot].mem.is_some() {
            return;
        }
        st.tick += 1;
        let size = chunk.size_bytes();
        let s = &mut st.slots[slot];
        s.mem = Some(chunk);
        s.size = size;
        s.last_used = st.tick;
        st.resident_bytes += size;
        self.enforce_limit(st);
    }

    /// チャンクを、必要なら呼び出したスレッドでファイルから読んで返す。
    pub(crate) fn chunk_blocking(self: &Arc<Self>, slot: usize) -> Result<Arc<Chunk>, StoreError> {
        let (file, region) = {
            let mut st = self.lock();
            st.tick += 1;
            let tick = st.tick;
            let s = &mut st.slots[slot];
            s.last_used = tick;
            if let Some(chunk) = &s.mem {
                return Ok(Arc::clone(chunk));
            }
            let region = s.file.expect("non-resident chunk is on disk");
            (st.file.clone().expect("spill file exists"), region)
        };
        let chunk = Arc::new(Chunk::decode(&file.read(region)?)?);
        let mut st = self.lock();
        if let Some(existing) = &st.slots[slot].mem {
            return Ok(Arc::clone(existing));
        }
        self.install(&mut st, slot, Arc::clone(&chunk));
        Ok(chunk)
    }
}

fn io_loop(inner: Weak<Inner>, jobs: Receiver<Job>) {
    while let Ok(job) = jobs.recv() {
        let Some(inner) = inner.upgrade() else { return };
        match job {
            Job::Write { slot, chunk } => inner.complete_write(slot, &chunk),
            Job::Read { slot } => inner.complete_read(slot),
        }
    }
}
