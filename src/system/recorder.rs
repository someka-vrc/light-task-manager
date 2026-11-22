//! Recording to the database: while it is on, samples are written to a SQLite file per
//! day (by default under %LOCALAPPDATA%\LightTaskManager\history), so that other programs
//! can query them, and the last seconds before a crash survive. The settings apply at the
//! next start of the app, so the recording never changes while it runs.
//!
//! The sampler thread hands each sample to the writer thread through a channel, so a
//! stuck window or a slow disk never delays the sampling, and the other way around.
//! See `.local/DB常時書き出し.md` for the design.

use crate::system::model::{Snapshot, Status};
use crate::system::sqlite::{api, Db, Stmt};
use crate::system::win::{civil_from_days, days_from_civil, filetime_now};
use std::collections::HashSet;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, sync_channel, SyncSender, TrySendError};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::SYSTEMTIME;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;

/// Samples measured in this long after start-up are not recorded: they mostly show
/// this app's own start-up.
const WARMUP: Duration = Duration::from_secs(2);
/// After the database could not be opened, wait this long before trying again.
const RETRY: Duration = Duration::from_secs(30);
/// How long the app waits for the last rows to be written when it ends.
const EXIT_WAIT: Duration = Duration::from_secs(3);
/// How many samples may wait for the writer. When the disk stalls, further samples are
/// dropped rather than piling up in memory.
const QUEUE: usize = 8;

/// The version of the tables, in the names of the files: a file is only ever written by the
/// version of the tables that made it. Raise it when a table changes in a way that old files
/// cannot take (a new column, say), and keep `session` (its `session_id`, `ended_at` and
/// `end_reason`) and `sample.ts` as they are: the start-up pass reads those in files of every version.
pub const SCHEMA_VERSION: u32 = 1;

/// Whether the last attempt to write failed: the folder is missing or cannot be written, the
/// disk is full... The settings page shows it. It clears when a write succeeds again.
pub static FAILED: AtomicBool = AtomicBool::new(false);

/// What the writer needs to know besides the sample; it is fixed when the app starts.
#[derive(Clone, Copy)]
pub struct Cfg {
    /// Record the path, command line and user name of each process.
    pub detail: bool,
    /// Record the GPU use.
    pub gpu: bool,
    /// How many days of files are kept, today included.
    pub days: u32,
}

/// What the settings say at the start of the app: where, how often and what to record.
pub struct Plan {
    pub ms: i64,
    pub dir: PathBuf,
    pub cfg: Cfg,
}

/// The plan for the settings, or None when nothing is recorded (it is off, or there is no
/// folder to write to: recording somewhere unexpected would be worse than not recording).
pub fn plan(s: &crate::settings::Settings) -> Option<Plan> {
    if !s.rec_enabled {
        return None;
    }
    Some(Plan {
        ms: s.rec_seconds.max(1) as i64 * 1000,
        dir: folder(&s.rec_dir)?,
        cfg: Cfg { detail: s.rec_detail, gpu: s.rec_gpu, days: s.rec_days.max(1) },
    })
}

pub enum Msg {
    Sample(Arc<Snapshot>),
    /// The app is ending (`shutdown`: Windows is shutting down). Answers when done.
    Exit { shutdown: bool, done: std::sync::mpsc::Sender<()> },
}

static START: OnceLock<Instant> = OnceLock::new();

/// Starts the writer thread. It sleeps until a sample arrives.
pub fn start(plan: Plan) -> SyncSender<Msg> {
    START.get_or_init(Instant::now);
    let (tx, rx) = sync_channel::<Msg>(QUEUE);
    std::thread::Builder::new()
        .name("recorder".into())
        .spawn(move || {
            let mut writer = Writer::new(plan.dir);
            // After the app ended its session, samples that are still on their way are dropped:
            // one would start a session that nothing ends.
            let mut closed = false;
            for msg in rx {
                match msg {
                    Msg::Sample(snap) => {
                        if !closed && !in_warmup(&snap) {
                            writer.sample(&snap, plan.cfg, local_date());
                            FAILED.store(writer.failed, Ordering::Relaxed);
                        }
                    }
                    Msg::Exit { shutdown, done } => {
                        closed = true;
                        writer.end(if shutdown { "shutdown" } else { "exit" });
                        let _ = done.send(());
                    }
                }
            }
        })
        .ok();
    tx
}

/// Hands a message to the writer without waiting; false when its queue is full.
pub fn offer(tx: &SyncSender<Msg>, msg: Msg) -> bool {
    !matches!(tx.try_send(msg), Err(TrySendError::Full(_)))
}

/// Writes the end of the session and waits (briefly) until it is on disk.
pub fn shutdown(tx: &SyncSender<Msg>, shutdown: bool) {
    let (done, wait) = channel();
    let deadline = Instant::now() + EXIT_WAIT;
    let mut msg = Msg::Exit { shutdown, done };
    // The queue may be full while the writer is slow: keep offering until the deadline.
    loop {
        match tx.try_send(msg) {
            Ok(()) => break,
            Err(TrySendError::Full(m)) if Instant::now() < deadline => {
                msg = m;
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => return,
        }
    }
    let _ = wait.recv_timeout(deadline.saturating_duration_since(Instant::now()));
}

fn in_warmup(snap: &Snapshot) -> bool {
    if snap.sys.elapsed <= 0.0 {
        return true;
    }
    let Some(start) = START.get() else { return false };
    let began = Instant::now().checked_sub(Duration::from_secs_f32(snap.sys.elapsed));
    began.is_none_or(|t| t < *start + WARMUP)
}

/// The folder the files go to by default, or None where there is no per-user data folder.
pub fn default_dir() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty())?;
    Some(PathBuf::from(base).join("LightTaskManager").join("history"))
}

/// The folder of the files for the setting: the default for an empty one.
pub fn folder(custom: &str) -> Option<PathBuf> {
    if custom.is_empty() {
        default_dir()
    } else {
        Some(PathBuf::from(custom))
    }
}

/// Whether a folder holds anything at all (the settings page asks before using one that does).
pub fn has_content(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut d| d.next().is_some())
}

// ---------------------------------------------------------------- time

/// FILETIME (UTC) to Unix milliseconds.
pub fn unix_ms(filetime: i64) -> i64 {
    filetime / 10_000 - 11_644_473_600_000
}

/// Today's date in local time as YYYYMMDD.
fn local_date() -> i32 {
    let mut t: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut t) };
    t.wYear as i32 * 10000 + t.wMonth as i32 * 100 + t.wDay as i32
}

/// A YYYYMMDD date moved by some days.
fn date_add(date: i32, days: i64) -> i32 {
    let (y, m, d) = (date as i64 / 10000, date as i64 / 100 % 100, date as i64 % 100);
    let (y, m, d) = civil_from_days(days_from_civil(y, m, d) + days);
    (y * 10000 + m * 100 + d) as i32
}

// ---------------------------------------------------------------- files

fn file_path(dir: &Path, date: i32) -> PathBuf {
    dir.join(format!("history-v{SCHEMA_VERSION}-{date:08}.db"))
}

/// The date in the name of a history file of any version, like `history-v1-20261117.db`. Nothing
/// else: the folder may be one the user chose, with files of their own, and only ours are deleted.
fn file_date(name: &str) -> Option<i32> {
    let rest = name.strip_prefix("history-v")?.strip_suffix(".db")?;
    let (version, digits) = rest.split_once('-')?;
    if version.is_empty() || !version.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if digits.len() != 8 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// The history files in a folder, of every version, oldest first, by the date in their names.
fn list_files(dir: &Path) -> Vec<(i32, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<(i32, PathBuf)> = entries
        .filter_map(|e| {
            let path = e.ok()?.path();
            let date = file_date(path.file_name()?.to_str()?)?;
            Some((date, path))
        })
        .collect();
    out.sort();
    out
}

fn side_file(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Deletes the files older than the retention period, but never those of the newest date: a
/// clock that jumped ahead must not wipe every file. Only files with the names of this feature
/// are touched, whatever else the folder holds.
fn cleanup(dir: &Path, days: u32, today: i32) {
    let files = list_files(dir);
    let newest = files.last().map_or(0, |f| f.0);
    let cutoff = date_add(today, -(days.max(1) as i64 - 1));
    for (date, path) in &files {
        if *date < cutoff && *date != newest {
            // A file somebody holds open stays; the next pass tries again.
            if std::fs::remove_file(path).is_ok() {
                let _ = std::fs::remove_file(side_file(path, "-wal"));
                let _ = std::fs::remove_file(side_file(path, "-shm"));
            }
        }
    }
}

const END_ABNORMAL: &str = "UPDATE session SET end_reason = 'abnormal', \
     ended_at = COALESCE((SELECT max(ts) FROM sample WHERE sample.session_id = session.session_id), began_at) \
     WHERE ended_at IS NULL";

/// At the start: files that still have a `-wal` are folded into their database, so the
/// last seconds of a crashed day are in the `.db` itself; and the open segments of the newest
/// date's files (of any version) belong to a run that ended abnormally.
fn startup_pass(dir: &Path) {
    let files = list_files(dir);
    let newest = files.last().map_or(0, |f| f.0);
    for (date, path) in &files {
        let leftover = side_file(path, "-wal").exists();
        if *date != newest && !leftover {
            continue;
        }
        let Ok(db) = Db::open(path) else { continue };
        let _ = db.exec(END_ABNORMAL);
        let _ = db.exec("PRAGMA wal_checkpoint(TRUNCATE)");
    }
}

// ---------------------------------------------------------------- schema

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
PRAGMA journal_size_limit = 67108864;
CREATE TABLE IF NOT EXISTS session (
  session_id   INTEGER NOT NULL,
  began_at     INTEGER NOT NULL,
  ended_at     INTEGER,
  end_reason   TEXT,
  app_version  TEXT NOT NULL,
  os_version   TEXT,
  PRIMARY KEY (session_id, began_at)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS sample (
  ts                         INTEGER PRIMARY KEY,
  session_id                 INTEGER NOT NULL,
  uptime_ms                  INTEGER NOT NULL,
  elapsed_seconds            REAL,
  cpu_ratio                  REAL,
  cpu_kernel_ratio           REAL,
  cpu_clock_hz               REAL,
  memory_used_bytes          INTEGER,
  memory_total_bytes         INTEGER,
  commit_bytes               INTEGER,
  commit_limit_bytes         INTEGER,
  handles                    INTEGER,
  page_faults_per_second     REAL,
  not_responding_processes   INTEGER
);
CREATE TABLE IF NOT EXISTS process (
  pid          INTEGER NOT NULL,
  create_time  INTEGER NOT NULL,
  ppid         INTEGER,
  name         TEXT NOT NULL,
  path         TEXT,
  cmdline      TEXT,
  user         TEXT,
  PRIMARY KEY (pid, create_time)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS process_sample (
  ts                      INTEGER NOT NULL,
  pid                     INTEGER NOT NULL,
  create_time             INTEGER NOT NULL,
  -- CPU
  cpu_ratio               REAL,
  cpu_time_seconds        REAL,     -- cumulative
  cycles                  INTEGER,  -- cumulative
  -- memory
  memory_bytes            INTEGER,  -- the active private working set
  private_bytes           INTEGER,  -- the private working set
  working_set_bytes       INTEGER,
  commit_bytes            INTEGER,
  paged_pool_bytes        INTEGER,
  nonpaged_pool_bytes     INTEGER,
  page_faults             INTEGER,  -- cumulative
  page_faults_per_second  REAL,
  -- disk and I/O
  disk_bytes_per_second   INTEGER,
  io_read_bytes           INTEGER,  -- cumulative
  io_write_bytes          INTEGER,  -- cumulative
  io_read_count           INTEGER,  -- cumulative
  io_write_count          INTEGER,  -- cumulative
  io_other_count          INTEGER,  -- cumulative
  -- objects
  handles                 INTEGER,
  threads                 INTEGER,
  user_objects            INTEGER,
  gdi_objects             INTEGER,
  -- GPU (only while the GPU recording is on)
  gpu_ratio               REAL,
  gpu_memory_bytes        INTEGER,  -- dedicated GPU memory
  gpu_shared_bytes        INTEGER,  -- shared GPU memory
  gpu_luid                TEXT,     -- the adapter of the busiest engine; `device.key` where kind = 'gpu'
  gpu_engine              TEXT,     -- the type of the busiest engine, such as 3D
  -- state
  not_responding          INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (ts, pid, create_time)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS device (
  kind         TEXT NOT NULL CHECK (kind IN ('disk','gpu')),
  key          TEXT NOT NULL,
  name         TEXT,
  total_bytes  INTEGER,
  PRIMARY KEY (kind, key)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS device_sample (
  ts            INTEGER NOT NULL,
  kind          TEXT NOT NULL,
  key           TEXT NOT NULL,
  util_ratio    REAL,
  memory_bytes  INTEGER,
  PRIMARY KEY (ts, kind, key)
) WITHOUT ROWID;
CREATE VIEW IF NOT EXISTS v_process_sample AS
SELECT datetime(ps.ts / 1000, 'unixepoch', 'localtime') AS time_local, p.name, ps.*
FROM process_sample ps LEFT JOIN process p USING (pid, create_time);
PRAGMA user_version = 1;
";

const INSERT_SAMPLE: &str = "INSERT OR IGNORE INTO sample (ts, session_id, uptime_ms, elapsed_seconds, cpu_ratio, \
    cpu_kernel_ratio, cpu_clock_hz, memory_used_bytes, memory_total_bytes, commit_bytes, commit_limit_bytes, handles, \
    page_faults_per_second, not_responding_processes) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)";
const INSERT_PROCESS: &str = "INSERT OR IGNORE INTO process (pid, create_time, ppid, name, path, cmdline, user) \
    VALUES (?1,?2,?3,?4,?5,?6,?7)";
const INSERT_PROCESS_SAMPLE: &str = "INSERT OR IGNORE INTO process_sample (ts, pid, create_time, cpu_ratio, cpu_time_seconds,     cycles, memory_bytes, private_bytes, working_set_bytes, commit_bytes, paged_pool_bytes, nonpaged_pool_bytes, page_faults,     page_faults_per_second, disk_bytes_per_second, io_read_bytes, io_write_bytes, io_read_count, io_write_count, io_other_count,     handles, threads, user_objects, gdi_objects, gpu_ratio, gpu_memory_bytes, gpu_shared_bytes, gpu_luid, gpu_engine, not_responding)     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,?30)";
const INSERT_DEVICE: &str = "INSERT OR IGNORE INTO device (kind, key, name, total_bytes) VALUES (?1,?2,?3,?4)";
const INSERT_DEVICE_SAMPLE: &str =
    "INSERT OR IGNORE INTO device_sample (ts, kind, key, util_ratio, memory_bytes) VALUES (?1,?2,?3,?4,?5)";

// ---------------------------------------------------------------- writer

/// The statements of one open file. They are listed before the connection so that they
/// are finalized first when the file closes.
struct Stmts {
    sample: Stmt,
    process: Stmt,
    process_sample: Stmt,
    device: Stmt,
    device_sample: Stmt,
}

/// One day's file, open for writing.
struct Open {
    stmts: Stmts,
    db: Db,
    date: i32,
    /// Processes and devices already written to this file.
    processes: HashSet<(u32, i64)>,
    devices: HashSet<(&'static str, String)>,
}

pub struct Writer {
    dir: PathBuf,
    open: Option<Open>,
    /// Held while recording, so that two writers never share a folder.
    lock: Option<std::fs::File>,
    session: i64,
    last_ts: i64,
    /// The date (YYYYMMDD) of the file that is, or was last, open.
    date: i32,
    active: bool,
    /// The last attempt to write failed.
    failed: bool,
    retry_at: Option<Instant>,
}

fn run(st: &Stmt) -> Result<(), String> {
    let r = st.step();
    st.reset();
    r.map(|_| ()).map_err(|rc| format!("step failed ({rc})"))
}

impl Writer {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir, open: None, lock: None, session: 0, last_ts: 0, date: 0, active: false, failed: false, retry_at: None }
    }

    /// Writes one sample. `today` is the local date, as YYYYMMDD.
    pub fn sample(&mut self, snap: &Snapshot, cfg: Cfg, today: i32) {
        if api().is_none() {
            self.failed = true;
            return;
        }
        // A stale sample only renews the baselines of the rates, so its rates are all zero.
        if snap.taken == 0 || !snap.has_procs || snap.stale {
            return;
        }
        let ts = unix_ms(snap.taken);
        if !self.active {
            if self.retry_at.is_some_and(|t| Instant::now() < t) {
                return;
            }
            if !self.begin(ts, today, cfg) {
                self.retry_at = Some(Instant::now() + RETRY);
                self.failed = true;
                return;
            }
        } else if self.open.as_ref().is_none_or(|o| o.date < today) {
            self.reopen(ts, today, cfg);
        }
        let Some(open) = self.open.as_mut() else { return };
        match write_rows(open, snap, cfg, self.session, ts) {
            Ok(()) => {
                self.last_ts = ts;
                self.failed = false;
            }
            // The file is opened again at the next sample.
            Err(_) => {
                self.open = None;
                self.failed = true;
            }
        }
    }

    fn begin(&mut self, ts: i64, today: i32, cfg: Cfg) -> bool {
        if std::fs::create_dir_all(&self.dir).is_err() {
            return false;
        }
        let lock = std::fs::OpenOptions::new().write(true).create(true).truncate(false).share_mode(0).open(self.dir.join(".lock"));
        let Ok(lock) = lock else { return false };
        startup_pass(&self.dir);
        match open_file(&self.dir, today, ts, ts, true) {
            Ok(open) => {
                self.lock = Some(lock);
                self.open = Some(open);
                self.session = ts;
                self.date = today;
                self.active = true;
                self.retry_at = None;
                // After opening today's file, so that the file kept as the newest is today's.
                cleanup(&self.dir, cfg.days, today);
                true
            }
            Err(_) => false,
        }
    }

    /// Opens the file of `today`; the old one, when there is one, ends its segment.
    fn reopen(&mut self, ts: i64, today: i32, cfg: Cfg) {
        let rotated = self.open.is_some();
        if rotated {
            self.close_segment("rotate", self.last_ts.max(self.session));
            if let Some(old) = self.open.take() {
                let _ = old.db.exec("PRAGMA wal_checkpoint(TRUNCATE)");
            }
        }
        // A new day needs a new segment; a file that is opened again after an error has its own.
        let segment = self.date != today;
        self.date = today;
        self.open = open_file(&self.dir, today, self.session, ts, segment).ok();
        if rotated && self.open.is_some() {
            cleanup(&self.dir, cfg.days, today);
        }
    }

    fn close_segment(&mut self, reason: &str, at: i64) {
        if let Some(open) = &self.open {
            let _ = open.db.exec(&format!(
                "UPDATE session SET ended_at = {at}, end_reason = '{reason}' WHERE session_id = {} AND ended_at IS NULL",
                self.session
            ));
        }
    }

    /// Ends the session: the app is ending.
    pub fn end(&mut self, reason: &str) {
        if !self.active {
            return;
        }
        let now = unix_ms(filetime_now());
        // After a write error there is no open file: open it again to write the end.
        if self.open.is_none() {
            self.open = open_file(&self.dir, self.date, self.session, now, false).ok();
        }
        self.close_segment(reason, now);
        if let Some(open) = self.open.take() {
            let _ = open.db.exec("PRAGMA wal_checkpoint(TRUNCATE)");
        }
        self.lock = None;
        self.active = false;
    }
}

/// Opens (and prepares) the file of a date. With `segment`, the session gets its segment in it,
/// beginning at `began`; opening the same file again does not add one.
fn open_file(dir: &Path, date: i32, session: i64, began: i64, segment: bool) -> Result<Open, String> {
    let db = Db::open(&file_path(dir, date))?;
    db.exec(SCHEMA)?;
    if segment {
        db.exec(&format!(
            "INSERT OR IGNORE INTO session (session_id, began_at, app_version) VALUES ({session}, {began}, '{}')",
            env!("CARGO_PKG_VERSION")
        ))?;
    }
    let stmts = Stmts {
        sample: db.prepare(INSERT_SAMPLE)?,
        process: db.prepare(INSERT_PROCESS)?,
        process_sample: db.prepare(INSERT_PROCESS_SAMPLE)?,
        device: db.prepare(INSERT_DEVICE)?,
        device_sample: db.prepare(INSERT_DEVICE_SAMPLE)?,
    };
    Ok(Open { stmts, db, date, processes: HashSet::new(), devices: HashSet::new() })
}

/// One transaction per sample, so that a crash leaves whole samples only.
fn write_rows(open: &mut Open, snap: &Snapshot, cfg: Cfg, session: i64, ts: i64) -> Result<(), String> {
    open.db.exec("BEGIN")?;
    match rows(open, snap, cfg, session, ts) {
        Ok(()) => open.db.exec("COMMIT"),
        Err(e) => {
            let _ = open.db.exec("ROLLBACK");
            Err(e)
        }
    }
}

fn rows(open: &mut Open, snap: &Snapshot, cfg: Cfg, session: i64, ts: i64) -> Result<(), String> {
    let Open { stmts, processes, devices, .. } = open;
    let sys = &snap.sys;
    let secs = sys.elapsed.max(0.001) as f64;
    let procs = || snap.procs.iter().filter(|p| p.pid != 0);
    let faults = procs().map(|p| p.pf_delta as f64).sum::<f64>() / secs;
    let hung = procs().filter(|p| p.status == Status::NotResponding).count();

    let s = &stmts.sample;
    s.bind_i64(1, ts);
    s.bind_i64(2, session);
    s.bind_i64(3, sys.cpu.uptime_ms as i64);
    s.bind_f64(4, sys.elapsed as f64);
    s.bind_f64(5, sys.cpu.total as f64 / 100.0);
    s.bind_f64(6, sys.cpu.kernel as f64 / 100.0);
    s.bind_f64(7, sys.cpu.mhz as f64 * 1e6);
    s.bind_i64(8, sys.mem.total.saturating_sub(sys.mem.available) as i64);
    s.bind_i64(9, sys.mem.total as i64);
    s.bind_i64(10, sys.mem.commit as i64);
    s.bind_i64(11, sys.mem.commit_limit as i64);
    s.bind_i64(12, sys.cpu.handles as i64);
    s.bind_f64(13, faults);
    s.bind_i64(14, hung as i64);
    run(s)?;

    for p in procs() {
        let create_time = if p.create_time > 0 { unix_ms(p.create_time) } else { 0 };
        if processes.insert((p.pid, p.create_time)) {
            let st = &stmts.process;
            st.bind_i64(1, p.pid as i64);
            st.bind_i64(2, create_time);
            st.bind_i64(3, p.ppid as i64);
            st.bind_text(4, &p.stat.name);
            if cfg.detail {
                st.bind_opt_text(5, Some(p.stat.path()).filter(|v| !v.is_empty()));
                st.bind_opt_text(6, Some(p.stat.cmdline.as_str()).filter(|v| !v.is_empty()));
                st.bind_opt_text(7, Some(&*p.stat.user));
            } else {
                st.bind_null(5);
                st.bind_null(6);
                st.bind_null(7);
            }
            run(st)?;
        }
        let st = &stmts.process_sample;
        st.bind_i64(1, ts);
        st.bind_i64(2, p.pid as i64);
        st.bind_i64(3, create_time);
        st.bind_f64(4, p.cpu as f64 / 100.0);
        st.bind_f64(5, p.cpu_time as f64 / 1e7);
        st.bind_i64(6, p.cycles as i64);
        st.bind_i64(7, p.active_private as i64);
        st.bind_i64(8, p.private_ws as i64);
        st.bind_i64(9, p.ws as i64);
        st.bind_i64(10, p.commit as i64);
        st.bind_i64(11, p.paged as i64);
        st.bind_i64(12, p.nonpaged as i64);
        st.bind_i64(13, p.page_faults as i64);
        st.bind_f64(14, p.pf_delta as f64 / secs);
        st.bind_i64(15, p.disk_bps as i64);
        st.bind_i64(16, p.io_read_bytes as i64);
        st.bind_i64(17, p.io_write_bytes as i64);
        st.bind_i64(18, p.io_reads as i64);
        st.bind_i64(19, p.io_writes as i64);
        st.bind_i64(20, p.io_other as i64);
        st.bind_i64(21, p.handles as i64);
        st.bind_i64(22, p.threads as i64);
        st.bind_i64(23, p.user_objects as i64);
        st.bind_i64(24, p.gdi_objects as i64);
        st.bind_opt_f64(25, cfg.gpu.then_some(p.gpu_util as f64 / 100.0));
        st.bind_opt_i64(26, cfg.gpu.then_some(p.gpu_dedicated as i64));
        st.bind_opt_i64(27, cfg.gpu.then_some(p.gpu_shared as i64));
        st.bind_opt_text(28, cfg.gpu.then_some(p.gpu_luid.as_str()).filter(|v| !v.is_empty()));
        st.bind_opt_text(29, cfg.gpu.then_some(p.gpu_kind.as_str()).filter(|v| !v.is_empty()));
        st.bind_i64(30, (p.status == Status::NotResponding) as i64);
        run(st)?;
    }

    for d in &sys.disks {
        let key = d.index.to_string();
        if devices.insert(("disk", key.clone())) {
            let name = format!("{} {}", d.info.model, d.letters);
            let st = &stmts.device;
            st.bind_text(1, "disk");
            st.bind_text(2, &key);
            st.bind_text(3, name.trim());
            st.bind_null(4);
            run(st)?;
        }
        let st = &stmts.device_sample;
        st.bind_i64(1, ts);
        st.bind_text(2, "disk");
        st.bind_text(3, &key);
        st.bind_f64(4, d.active as f64 / 100.0);
        st.bind_null(5);
        run(st)?;
    }
    if cfg.gpu {
        for g in &sys.gpus {
            if devices.insert(("gpu", g.luid.clone())) {
                let st = &stmts.device;
                st.bind_text(1, "gpu");
                st.bind_text(2, &g.luid);
                st.bind_text(3, &g.name);
                st.bind_i64(4, g.dedicated_total as i64);
                run(st)?;
            }
            let st = &stmts.device_sample;
            st.bind_i64(1, ts);
            st.bind_text(2, "gpu");
            st.bind_text(3, &g.luid);
            st.bind_f64(4, g.util as f64 / 100.0);
            st.bind_i64(5, g.dedicated as i64);
            run(st)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::model::{DiskSample, GpuSample, Proc, ProcStatic, Protect};
    use crate::system::win::SEC;

    const BASE: i64 = 133_000_000_000_000_000;
    const CFG: Cfg = Cfg { detail: false, gpu: false, days: 3 };

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ltm-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn proc(pid: u32, cpu: f32) -> Proc {
        Proc {
            pid,
            ppid: 4,
            create_time: BASE - SEC * pid as i64,
            stat: Arc::new(ProcStatic {
                name: format!("p{pid}.exe"),
                user: Arc::from("someone"),
                arch: "x64",
                elevated: None,
                cmdline: format!("p{pid}.exe --flag"),
                image: None,
                package: None,
                platform: 64,
                system_dir: false,
                protect: Protect::default(),
            }),
            status: if pid == 3 { Status::NotResponding } else { Status::Running },
            session: 1,
            has_window: false,
            cpu,
            cpu_time: 25_000_000,
            cycles: 99,
            ws: 5000,
            peak_ws: 0,
            ws_delta: 0,
            disk_bps: 2048,
            gpu_util: 12.5,
            gpu_engine: "GPU 0 - 3D".into(),
            gpu_luid: "0x1".into(),
            gpu_kind: "3D".into(),
            gpu_dedicated: 4096,
            gpu_shared: 555,
            net_bps: 0,
            eco_qos: None,
            private_ws: 3000,
            active_private: 1_000_000,
            commit: 2_000_000,
            paged: 11,
            nonpaged: 12,
            page_faults: 77,
            pf_delta: 10,
            base_priority: 8,
            handles: 100 + pid,
            threads: 4,
            user_objects: 5,
            gdi_objects: 6,
            io_reads: 7,
            io_writes: 8,
            io_other: 9,
            io_read_bytes: 111,
            io_write_bytes: 222,
            io_other_bytes: 0,
        }
    }

    fn snap(n: i64) -> Snapshot {
        let mut s = Snapshot { taken: BASE + n * 5 * SEC, has_procs: true, ..Default::default() };
        s.procs = vec![proc(0, 90.0), proc(2, 10.0), proc(3, 20.0)];
        s.sys.elapsed = 5.0;
        s.sys.cpu.total = 25.0;
        s.sys.cpu.uptime_ms = 1_000_000 + n as u64 * 5000;
        s.sys.mem.total = 16_000;
        s.sys.mem.available = 6_000;
        s.sys.disks = vec![DiskSample { index: 0, letters: "C:".into(), active: 50.0, ..Default::default() }];
        s.sys.gpus = vec![GpuSample { luid: "0x1".into(), name: "GPU".into(), util: 40.0, dedicated: 5, dedicated_total: 10, ..Default::default() }];
        s
    }

    fn count(path: &Path, sql: &str) -> i64 {
        Db::open(path).unwrap().query_i64(sql).unwrap_or(-1)
    }

    fn sql(dir: &Path, date: i32, q: &str) -> i64 {
        count(&file_path(dir, date), q)
    }

    #[test]
    fn dates() {
        assert_eq!(date_add(20261117, -2), 20261115);
        assert_eq!(date_add(20260301, -1), 20260228);
        assert_eq!(date_add(20240301, -1), 20240229);
        assert_eq!(date_add(20270101, -1), 20261231);
        assert_eq!(unix_ms(116_444_736_000_000_000), 0);
    }

    #[test]
    fn writes_samples_processes_and_devices() {
        let dir = temp_dir("write");
        let mut w = Writer::new(dir.clone());
        w.sample(&snap(0), CFG, 20261117);
        w.sample(&snap(1), CFG, 20261117);
        w.end("exit");
        let q = |q: &str| sql(&dir, 20261117, q);
        assert_eq!(q("SELECT count(*) FROM sample"), 2);
        // Process 0 is left out; 2 and 3 are written once to `process`, in both samples.
        assert_eq!(q("SELECT count(*) FROM process"), 2);
        assert_eq!(q("SELECT count(*) FROM process_sample"), 4);
        assert_eq!(q("SELECT count(*) FROM device_sample WHERE kind = 'disk'"), 2);
        // GPU recording is off.
        assert_eq!(q("SELECT count(*) FROM device_sample WHERE kind = 'gpu'"), 0);
        assert_eq!(q("SELECT count(*) FROM process_sample WHERE gpu_ratio IS NOT NULL"), 0);
        // Details are off.
        assert_eq!(q("SELECT count(*) FROM process WHERE path IS NOT NULL OR cmdline IS NOT NULL OR user IS NOT NULL"), 0);
        assert_eq!(q("SELECT not_responding_processes FROM sample LIMIT 1"), 1);
        assert_eq!(q("SELECT count(*) FROM session WHERE end_reason = 'exit' AND ended_at IS NOT NULL"), 1);
        assert_eq!(q("SELECT count(*) FROM process_sample WHERE cpu_time_seconds = 2.5 AND io_write_bytes = 222"), 4);
        assert_eq!(q("SELECT memory_used_bytes FROM sample LIMIT 1"), 10_000);
        // The memory, object, cycle and I/O count columns.
        assert_eq!(
            q("SELECT count(*) FROM process_sample WHERE working_set_bytes = 5000 AND private_bytes = 3000 AND paged_pool_bytes = 11                AND nonpaged_pool_bytes = 12 AND threads = 4 AND user_objects = 5 AND gdi_objects = 6 AND cycles = 99                AND io_read_count = 7 AND io_write_count = 8 AND io_other_count = 9"),
            4
        );
        // The GPU ones follow the GPU setting, which is off here.
        assert_eq!(q("SELECT count(*) FROM process_sample WHERE gpu_shared_bytes IS NOT NULL OR gpu_engine IS NOT NULL OR gpu_luid IS NOT NULL"), 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn details_and_gpu_follow_the_settings() {
        let dir = temp_dir("detail");
        let mut w = Writer::new(dir.clone());
        w.sample(&snap(0), Cfg { detail: true, gpu: true, days: 3 }, 20261117);
        w.end("exit");
        let q = |q: &str| sql(&dir, 20261117, q);
        // The path is empty in this test (no image), so it stays NULL even with details on.
        assert_eq!(q("SELECT count(*) FROM process WHERE path IS NULL AND cmdline = name || ' --flag' AND user = 'someone'"), 2);
        assert_eq!(q("SELECT count(*) FROM process_sample WHERE gpu_ratio = 0.125"), 2);
        assert_eq!(q("SELECT count(*) FROM process_sample WHERE gpu_shared_bytes = 555 AND gpu_engine = '3D' AND gpu_luid = '0x1'"), 2);
        assert_eq!(q("SELECT count(*) FROM device_sample WHERE kind = 'gpu'"), 1);
        assert_eq!(q("SELECT total_bytes FROM device WHERE kind = 'gpu'"), 10);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn same_time_twice_is_ignored() {
        let dir = temp_dir("dup");
        let mut w = Writer::new(dir.clone());
        w.sample(&snap(0), CFG, 20261117);
        w.sample(&snap(0), CFG, 20261117);
        w.end("exit");
        assert_eq!(sql(&dir, 20261117, "SELECT count(*) FROM sample"), 1);
        assert_eq!(sql(&dir, 20261117, "SELECT count(*) FROM process_sample"), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rotates_at_a_new_date() {
        let dir = temp_dir("rotate");
        let mut w = Writer::new(dir.clone());
        w.sample(&snap(0), CFG, 20261117);
        w.sample(&snap(1), CFG, 20261118);
        // A date that goes back does not go back to the closed file.
        w.sample(&snap(2), CFG, 20261117);
        w.end("exit");
        assert_eq!(sql(&dir, 20261117, "SELECT count(*) FROM sample"), 1);
        assert_eq!(sql(&dir, 20261118, "SELECT count(*) FROM sample"), 2);
        assert_eq!(sql(&dir, 20261117, "SELECT count(*) FROM session WHERE end_reason = 'rotate'"), 1);
        assert_eq!(sql(&dir, 20261118, "SELECT count(*) FROM session WHERE end_reason = 'exit'"), 1);
        // The same session continues in the new file.
        let first = sql(&dir, 20261117, "SELECT session_id FROM session");
        assert_eq!(first, sql(&dir, 20261118, "SELECT session_id FROM session"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_run_that_did_not_end_is_marked_abnormal_at_the_next_start() {
        let dir = temp_dir("crash");
        let mut w = Writer::new(dir.clone());
        w.sample(&snap(0), CFG, 20261117);
        w.sample(&snap(1), CFG, 20261117);
        // The process dies: nothing is written, the lock goes with it.
        drop(w);
        assert_eq!(sql(&dir, 20261117, "SELECT count(*) FROM session WHERE ended_at IS NULL"), 1);

        let mut w = Writer::new(dir.clone());
        w.sample(&snap(10), CFG, 20261118);
        let abnormal = sql(&dir, 20261117, "SELECT ended_at FROM session WHERE end_reason = 'abnormal'");
        assert_eq!(abnormal, unix_ms(snap(1).taken));
        // The new run is still open, not abnormal.
        assert_eq!(sql(&dir, 20261118, "SELECT count(*) FROM session WHERE ended_at IS NULL"), 1);
        w.end("exit");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_second_writer_cannot_share_the_folder() {
        let dir = temp_dir("lock");
        let mut a = Writer::new(dir.clone());
        a.sample(&snap(0), CFG, 20261117);
        let mut b = Writer::new(dir.clone());
        b.sample(&snap(1), CFG, 20261117);
        assert!(!b.active);
        a.end("exit");
        assert_eq!(sql(&dir, 20261117, "SELECT count(*) FROM sample"), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_old_file_goes_at_the_rotation_that_makes_it_old() {
        // One day kept: when the new day's file is open, yesterday's is no longer the newest.
        let dir = temp_dir("retain");
        let mut w = Writer::new(dir.clone());
        let one_day = Cfg { days: 1, ..CFG };
        w.sample(&snap(0), one_day, 20261117);
        w.sample(&snap(1), one_day, 20261118);
        let left: Vec<i32> = list_files(&dir).into_iter().map(|f| f.0).collect();
        assert_eq!(left, vec![20261118]);
        w.end("exit");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_session_stopped_after_a_write_error_still_gets_its_end() {
        let dir = temp_dir("stopped");
        let mut w = Writer::new(dir.clone());
        w.sample(&snap(0), CFG, 20261117);
        // As after a failed write: no file is open.
        w.open = None;
        w.end("exit");
        assert_eq!(sql(&dir, 20261117, "SELECT count(*) FROM session WHERE ended_at IS NULL"), 0);
        assert_eq!(sql(&dir, 20261117, "SELECT count(*) FROM session WHERE end_reason = 'exit'"), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_stale_sample_is_not_recorded() {
        let dir = temp_dir("stale");
        let mut w = Writer::new(dir.clone());
        let mut stale = snap(0);
        stale.stale = true;
        w.sample(&stale, CFG, 20261117);
        assert!(!w.active);
        w.sample(&snap(1), CFG, 20261117);
        w.end("exit");
        assert_eq!(sql(&dir, 20261117, "SELECT count(*) FROM sample"), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn old_files_go_but_the_newest_stays() {
        let dir = temp_dir("clean");
        for d in [20261101, 20261114, 20261115, 20261117] {
            std::fs::write(file_path(&dir, d), b"x").unwrap();
        }
        // Not the names of this feature (the second is from a build before the version was in the name):
        // never touched.
        for other in ["other.txt", "history-20261102.db", "history-v-20261101.db", "history-v1-2026110.db", "history-v1-20261101.db.bak", "xhistory-20261101.db"] {
            std::fs::write(dir.join(other), b"x").unwrap();
        }
        // 3 days: today and the two before.
        cleanup(&dir, 3, 20261117);
        let left: Vec<i32> = list_files(&dir).into_iter().map(|f| f.0).collect();
        assert_eq!(left, vec![20261115, 20261117]);
        assert!(dir.join("other.txt").exists());
        assert!(dir.join("history-v-20261101.db").exists() && dir.join("history-v1-2026110.db").exists());
        assert!(dir.join("history-v1-20261101.db.bak").exists() && dir.join("xhistory-20261101.db").exists());
        // A clock far ahead would delete everything but the newest.
        cleanup(&dir, 3, 20300101);
        let left: Vec<i32> = list_files(&dir).into_iter().map(|f| f.0).collect();
        assert_eq!(left, vec![20261117]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn names_of_every_version_are_read_and_nothing_else() {
        assert_eq!(file_date("history-v1-20261117.db"), Some(20261117));
        assert_eq!(file_date("history-v12-20261117.db"), Some(20261117));
        for bad in ["history-20261117.db", "history-v1-20261117.db-wal", "history-v1-2026111.db", "history-v-20261117.db", "history-v1x-20261117.db", "history-.db", "a.db"] {
            assert_eq!(file_date(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_crash_of_an_older_version_is_marked_by_a_newer_one() {
        let dir = temp_dir("versions");
        // A file of another version, with a session that never ended.
        let old = dir.join("history-v0-20261117.db");
        {
            let db = Db::open(&old).unwrap();
            db.exec("CREATE TABLE session (session_id INTEGER, began_at INTEGER, ended_at INTEGER, end_reason TEXT, app_version TEXT);
                     CREATE TABLE sample (ts INTEGER, session_id INTEGER);
                     INSERT INTO session VALUES (1, 100, NULL, NULL, '0');
                     INSERT INTO sample VALUES (150, 1);").unwrap();
        }
        let mut w = Writer::new(dir.clone());
        w.sample(&snap(0), CFG, 20261118);
        assert_eq!(count(&old, "SELECT ended_at FROM session WHERE end_reason = 'abnormal'"), 150);
        // Today's file has the current version in its name and the tables of it.
        assert_eq!(sql(&dir, 20261118, "SELECT count(*) FROM sample"), 1);
        assert!(file_path(&dir, 20261118).to_str().unwrap().contains("-v1-"));
        w.end("exit");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_failure_shows_until_a_write_works() {
        let dir = temp_dir("failed");
        // A file where the folder should be: it cannot be made.
        let blocked = dir.join("history");
        std::fs::write(&blocked, b"x").unwrap();
        let mut w = Writer::new(blocked);
        w.sample(&snap(0), CFG, 20261117);
        assert!(w.failed);
        let mut w = Writer::new(dir.join("ok"));
        w.sample(&snap(0), CFG, 20261117);
        assert!(!w.failed);
        w.end("exit");
        let _ = std::fs::remove_dir_all(dir);
    }
}
