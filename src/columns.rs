//! Column definitions of the details table.

use crate::model::Proc;
use std::cmp::Ordering;
use std::fmt::Write;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Col {
    Name,
    Pid,
    Status,
    User,
    Cpu,
    WsDelta,
    ActivePrivate,
    Private,
    Arch,
    Description,
    Session,
    CpuTime,
    Cycles,
    Ws,
    PeakWs,
    SharedWs,
    Commit,
    Paged,
    NonPaged,
    PageFaults,
    PfDelta,
    Priority,
    Handles,
    Threads,
    UserObjects,
    GdiObjects,
    IoReads,
    IoWrites,
    IoOther,
    IoReadBytes,
    IoWriteBytes,
    IoOtherBytes,
    Path,
    CmdLine,
    Elevated,
}

pub struct ColDef {
    pub id: Col,
    pub key: &'static str,
    /// Short label for the header.
    pub label: &'static str,
    /// Full label for the column picker.
    pub full: &'static str,
    pub width: u16,
    pub right: bool,
}

const fn c(
    id: Col,
    key: &'static str,
    label: &'static str,
    full: &'static str,
    width: u16,
    right: bool,
) -> ColDef {
    ColDef { id, key, label, full, width, right }
}

/// Indexed by `Col as usize`.
pub const ALL: [ColDef; 35] = [
    c(Col::Name, "name", "名前", "名前", 230, false),
    c(Col::Pid, "pid", "PID", "PID", 72, true),
    c(Col::Status, "status", "状態", "状態", 80, false),
    c(Col::User, "user", "ユーザー名", "ユーザー名", 130, false),
    c(Col::Cpu, "cpu", "CPU", "CPU", 58, true),
    c(Col::WsDelta, "wsdelta", "WS の差分", "ワーキング セットの差分", 100, true),
    c(Col::ActivePrivate, "active", "メモリ (アクティブ)", "メモリ (アクティブなプライベート WS)", 140, true),
    c(Col::Private, "private", "メモリ (プライベート)", "メモリ (プライベート WS)", 146, true),
    c(Col::Arch, "arch", "アーキテクチャ", "アーキテクチャ", 110, false),
    c(Col::Description, "desc", "説明", "説明", 300, false),
    c(Col::Session, "session", "セッション", "セッション ID", 82, true),
    c(Col::CpuTime, "cputime", "CPU 時間", "CPU 時間", 92, true),
    c(Col::Cycles, "cycles", "サイクル", "サイクル", 150, true),
    c(Col::Ws, "ws", "ワーキング セット", "ワーキング セット", 130, true),
    c(Col::PeakWs, "peakws", "ピーク WS", "ピーク ワーキング セット", 110, true),
    c(Col::SharedWs, "sharedws", "メモリ (共有)", "メモリ (共有 WS)", 110, true),
    c(Col::Commit, "commit", "コミット サイズ", "コミット サイズ", 120, true),
    c(Col::Paged, "paged", "ページ プール", "ページ プール", 104, true),
    c(Col::NonPaged, "nppool", "NP プール", "NP プール", 96, true),
    c(Col::PageFaults, "pf", "ページ フォールト", "ページ フォールト", 124, true),
    c(Col::PfDelta, "pfdelta", "PF の差分", "PF の差分", 86, true),
    c(Col::Priority, "priority", "基本優先度", "基本優先度", 96, false),
    c(Col::Handles, "handles", "ハンドル", "ハンドル", 80, true),
    c(Col::Threads, "threads", "スレッド", "スレッド", 76, true),
    c(Col::UserObjects, "userobj", "ユーザー オブジェクト", "ユーザー オブジェクト", 150, true),
    c(Col::GdiObjects, "gdiobj", "GDI オブジェクト", "GDI オブジェクト", 120, true),
    c(Col::IoReads, "ioread", "I/O 読み取り", "I/O 読み取り", 110, true),
    c(Col::IoWrites, "iowrite", "I/O 書き込み", "I/O 書き込み", 110, true),
    c(Col::IoOther, "ioother", "I/O その他", "I/O その他", 110, true),
    c(Col::IoReadBytes, "ioreadb", "読み取りバイト", "I/O 読み取りバイト数", 130, true),
    c(Col::IoWriteBytes, "iowriteb", "書き込みバイト", "I/O 書き込みバイト数", 130, true),
    c(Col::IoOtherBytes, "iootherb", "その他バイト", "I/O その他バイト数", 130, true),
    c(Col::Path, "path", "イメージ パス名", "イメージ パス名", 360, false),
    c(Col::CmdLine, "cmdline", "コマンド ライン", "コマンド ライン", 420, false),
    c(Col::Elevated, "elevated", "管理者特権", "管理者特権", 90, false),
];

pub fn def(col: Col) -> &'static ColDef {
    &ALL[col as usize]
}

pub fn by_key(key: &str) -> Option<&'static ColDef> {
    ALL.iter().find(|d| d.key == key)
}

pub fn defaults() -> Vec<(Col, u16)> {
    [
        Col::Name,
        Col::Pid,
        Col::Status,
        Col::User,
        Col::Cpu,
        Col::WsDelta,
        Col::ActivePrivate,
        Col::Private,
        Col::Arch,
        Col::Description,
    ]
    .iter()
    .map(|c| (*c, def(*c).width))
    .collect()
}

fn is_text(col: Col) -> bool {
    matches!(
        col,
        Col::Name | Col::Status | Col::User | Col::Arch | Col::Description | Col::Path
            | Col::CmdLine | Col::Elevated
    )
}

/// Text columns open ascending, numeric columns descending.
pub fn default_descending(col: Col) -> bool {
    !is_text(col) && col != Col::Pid
}

/// Whether the column's cost requires per-process handle work.
pub fn needs_gui_counts(col: Col) -> bool {
    matches!(col, Col::UserObjects | Col::GdiObjects)
}

fn number(col: Col, p: &Proc) -> i64 {
    match col {
        Col::Pid => p.pid as i64,
        Col::Cpu => (p.cpu * 1000.0) as i64,
        Col::WsDelta => p.ws_delta,
        Col::ActivePrivate => p.active_private as i64,
        Col::Private => p.private_ws as i64,
        Col::Session => p.session as i64,
        Col::CpuTime => p.cpu_time as i64,
        Col::Cycles => p.cycles as i64,
        Col::Ws => p.ws as i64,
        Col::PeakWs => p.peak_ws as i64,
        Col::SharedWs => p.ws.saturating_sub(p.private_ws) as i64,
        Col::Commit => p.commit as i64,
        Col::Paged => p.paged as i64,
        Col::NonPaged => p.nonpaged as i64,
        Col::PageFaults => p.page_faults as i64,
        Col::PfDelta => p.pf_delta as i64,
        Col::Priority => p.base_priority as i64,
        Col::Handles => p.handles as i64,
        Col::Threads => p.threads as i64,
        Col::UserObjects => p.user_objects as i64,
        Col::GdiObjects => p.gdi_objects as i64,
        Col::IoReads => p.io_reads as i64,
        Col::IoWrites => p.io_writes as i64,
        Col::IoOther => p.io_other as i64,
        Col::IoReadBytes => p.io_read_bytes as i64,
        Col::IoWriteBytes => p.io_write_bytes as i64,
        Col::IoOtherBytes => p.io_other_bytes as i64,
        _ => 0,
    }
}

fn text(col: Col, p: &Proc) -> &str {
    match col {
        Col::Name => &p.stat.name,
        Col::Status => p.status.label(),
        Col::User => &p.stat.user,
        Col::Arch => p.stat.arch,
        Col::Description => p.stat.description(),
        Col::Path => p.stat.path(),
        Col::CmdLine => &p.stat.cmdline,
        Col::Elevated => elevated_label(p),
        _ => "",
    }
}

fn elevated_label(p: &Proc) -> &'static str {
    match p.stat.elevated {
        Some(true) => "はい",
        Some(false) => "いいえ",
        None if p.pid > 4 => "—",
        None => "",
    }
}

fn cmp_text(a: &str, b: &str) -> Ordering {
    a.bytes()
        .map(|c| c.to_ascii_lowercase())
        .cmp(b.bytes().map(|c| c.to_ascii_lowercase()))
}

pub fn compare(col: Col, a: &Proc, b: &Proc) -> Ordering {
    if is_text(col) {
        cmp_text(text(col, a), text(col, b))
    } else {
        number(col, a).cmp(&number(col, b))
    }
}

fn grouped(out: &mut String, value: i64) {
    if value < 0 {
        out.push('-');
    }
    let mut digits = [0u8; 24];
    let mut n = value.unsigned_abs();
    let mut i = digits.len();
    let mut count = 0;
    loop {
        if count == 3 {
            i -= 1;
            digits[i] = b',';
            count = 0;
        }
        i -= 1;
        digits[i] = b'0' + (n % 10) as u8;
        n /= 10;
        count += 1;
        if n == 0 {
            break;
        }
    }
    out.push_str(std::str::from_utf8(&digits[i..]).unwrap_or(""));
}

fn kilobytes(out: &mut String, bytes: i64) {
    grouped(out, bytes / 1024);
    out.push_str(" K");
}

fn priority_label(base: i32) -> &'static str {
    match base {
        0..=4 => "低",
        5..=6 => "通常以下",
        7..=8 => "通常",
        9..=10 => "通常以上",
        11..=13 => "高",
        _ => "リアルタイム",
    }
}

/// Appends the cell text to `out`.
pub fn cell(col: Col, p: &Proc, out: &mut String) {
    match col {
        Col::Cpu => {
            let _ = write!(out, "{:02}", p.cpu.round() as u32);
        }
        Col::CpuTime => {
            let seconds = p.cpu_time / 10_000_000;
            let _ = write!(
                out,
                "{}:{:02}:{:02}",
                seconds / 3600,
                seconds / 60 % 60,
                seconds % 60
            );
        }
        Col::Priority => out.push_str(if p.pid <= 4 { "" } else { priority_label(p.base_priority) }),
        Col::WsDelta | Col::ActivePrivate | Col::Private | Col::Ws | Col::PeakWs | Col::SharedWs
        | Col::Commit | Col::Paged | Col::NonPaged => kilobytes(out, number(col, p)),
        Col::Pid | Col::Session => {
            let _ = write!(out, "{}", number(col, p));
        }
        _ if is_text(col) => out.push_str(text(col, p)),
        _ => grouped(out, number(col, p)),
    }
}
