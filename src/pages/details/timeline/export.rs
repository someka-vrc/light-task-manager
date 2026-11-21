//! Writing the graph's samples to a file: the table as shown, the values of each
//! process at each sample, or the machine-wide values at each sample.
//!
//! Headings are snake_case with the unit at the end (`_bytes`, `_ratio`, `_seconds`,
//! `_per_second`, `_hz`); values are plain numbers (ratios from 0 to 1, sizes in bytes);
//! times are ISO 8601 with the offset. Paths, command lines and user names are never written.

use super::{PRec, Sample, SEC};
use crate::i18n::tr;
use crate::system::model::Status;
use std::io::{self, Write};
use std::sync::Arc;
use windows_sys::Win32::Globalization::{WideCharToMultiByte, WC_NO_BEST_FIT_CHARS};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// The table as shown: one row per process.
    Table,
    /// One row per sample and process.
    Process,
    /// One row per sample, machine-wide values.
    System,
}

impl Kind {
    /// In the order of the dialog's radio buttons.
    pub const ALL: [Kind; 3] = [Kind::Table, Kind::Process, Kind::System];

    pub fn key(self) -> &'static str {
        match self {
            Kind::Table => "table",
            Kind::Process => "process",
            Kind::System => "system",
        }
    }

    pub fn from_key(k: &str) -> Kind {
        Kind::ALL.into_iter().find(|x| x.key() == k).unwrap_or(Kind::Table)
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Table => tr().tm.kind_table,
            Kind::Process => tr().tm.kind_process,
            Kind::System => tr().tm.kind_system,
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Kind::Table => tr().tm.desc_table,
            Kind::Process => tr().tm.desc_process,
            Kind::System => tr().tm.desc_system,
        }
    }

    /// Whether the search box's filter changes what is written.
    pub fn filtered(self) -> bool {
        self != Kind::System
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    Csv,
    Tsv,
}

impl Format {
    pub const ALL: [Format; 2] = [Format::Csv, Format::Tsv];

    pub fn key(self) -> &'static str {
        match self {
            Format::Csv => "csv",
            Format::Tsv => "tsv",
        }
    }

    pub fn from_key(k: &str) -> Format {
        Format::ALL.into_iter().find(|x| x.key() == k).unwrap_or(Format::Csv)
    }

    pub fn label(self) -> &'static str {
        match self {
            Format::Csv => "CSV",
            Format::Tsv => "TSV",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Encoding {
    Utf8Bom,
    ShiftJis,
}

impl Encoding {
    pub const ALL: [Encoding; 2] = [Encoding::Utf8Bom, Encoding::ShiftJis];

    pub fn key(self) -> &'static str {
        match self {
            Encoding::Utf8Bom => "utf8",
            Encoding::ShiftJis => "sjis",
        }
    }

    pub fn from_key(k: &str) -> Encoding {
        Encoding::ALL.into_iter().find(|x| x.key() == k).unwrap_or(Encoding::Utf8Bom)
    }

    pub fn label(self) -> &'static str {
        match self {
            Encoding::Utf8Bom => "UTF-8 with BOM",
            Encoding::ShiftJis => "Shift_JIS",
        }
    }
}

/// A table to write as it is: headings and the cells of each row.
pub struct Table {
    pub head: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

/// Everything a file needs, taken from the page when export starts, so the file can be
/// written after the save dialog without the page.
pub struct Job {
    pub kind: Kind,
    pub format: Format,
    pub encoding: Encoding,
    /// The samples written (one for a single time).
    pub range: Vec<Arc<Sample>>,
    /// The table, built by the page: the one on screen, or the whole record summed up.
    pub table: Table,
    /// The search box's terms, lowercase, for process values.
    pub terms: Vec<String>,
    /// Local time minus UTC, in seconds.
    pub offset: i64,
    /// Disks: their number and their index in the samples.
    pub disks: Vec<(u32, usize)>,
    /// GPU adapters: their index in the samples, their number, and their dedicated memory.
    pub adapters: Vec<(usize, usize, u64)>,
}

/// Days since 1970-01-01 to a civil date.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y, m as u32, d as u32)
}

/// FILETIME seconds between 1601-01-01 and 1970-01-01.
const UNIX_FROM_1601: i64 = 11_644_473_600;

/// Local date and time parts of a FILETIME.
fn local_parts(ft: i64, offset: i64) -> (i64, u32, u32, i64, i64, i64) {
    let secs = ft.div_euclid(SEC) - UNIX_FROM_1601 + offset;
    let (y, m, d) = civil_from_days(secs.div_euclid(86400));
    let t = secs.rem_euclid(86400);
    (y, m, d, t / 3600, t / 60 % 60, t % 60)
}

/// ISO 8601 with the offset, like 2026-11-14T04:15:30+09:00.
pub fn iso(ft: i64, offset: i64) -> String {
    let (y, m, d, hh, mm, ss) = local_parts(ft, offset);
    let sign = if offset < 0 { '-' } else { '+' };
    let o = offset.abs();
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}{sign}{:02}:{:02}", o / 3600, o / 60 % 60)
}

/// A plain number with at most `decimals` places and no trailing zeros; empty when unknown.
pub fn number(v: f64, decimals: usize) -> String {
    if !v.is_finite() {
        return String::new();
    }
    let s = format!("{v:.decimals$}");
    let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s };
    if s == "-0" {
        "0".into()
    } else {
        s
    }
}

fn boolean(b: bool) -> String {
    (if b { "true" } else { "false" }).into()
}

/// Writes rows in the chosen format and encoding.
struct Out<'a> {
    w: &'a mut dyn Write,
    format: Format,
    encoding: Encoding,
}

impl Out<'_> {
    fn row(&mut self, fields: &[String]) -> io::Result<()> {
        let mut line = String::new();
        for (i, f) in fields.iter().enumerate() {
            if i > 0 {
                line.push(if self.format == Format::Csv { ',' } else { '\t' });
            }
            match self.format {
                Format::Csv if f.contains([',', '"', '\r', '\n']) => {
                    line.push('"');
                    line.push_str(&f.replace('"', "\"\""));
                    line.push('"');
                }
                Format::Csv => line.push_str(f),
                // TSV has no quoting: tabs and line breaks in a value become spaces.
                Format::Tsv => line.push_str(&f.replace(['\t', '\r', '\n'], " ")),
            }
        }
        line.push_str("\r\n");
        self.text(&line)
    }

    fn text(&mut self, s: &str) -> io::Result<()> {
        match self.encoding {
            Encoding::Utf8Bom => self.w.write_all(s.as_bytes()),
            Encoding::ShiftJis => self.w.write_all(&shift_jis(s)),
        }
    }
}

/// Shift_JIS (code page 932); characters it lacks become "?".
pub fn shift_jis(s: &str) -> Vec<u8> {
    let wide: Vec<u16> = s.encode_utf16().collect();
    if wide.is_empty() {
        return Vec::new();
    }
    unsafe {
        let n = WideCharToMultiByte(932, WC_NO_BEST_FIT_CHARS, wide.as_ptr(), wide.len() as i32, std::ptr::null_mut(), 0, std::ptr::null(), std::ptr::null_mut());
        let mut out = vec![0u8; n.max(0) as usize];
        WideCharToMultiByte(932, WC_NO_BEST_FIT_CHARS, wide.as_ptr(), wide.len() as i32, out.as_mut_ptr(), n, std::ptr::null(), std::ptr::null_mut());
        out
    }
}

/// Counts what would be written.
struct Count(u64);

impl Write for Count {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0 += buf.len() as u64;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Job {
    fn matches(&self, name: &str, pid: u32) -> bool {
        let hay = format!("{}\u{1}{pid}", name.to_lowercase());
        self.terms.iter().all(|t| hay.contains(t.as_str()))
    }

    /// The suggested file name: performance-{date}-{time}-{kind}.{ext}, at the start of the selection.
    pub fn file_name(&self) -> String {
        let start = self.range.first().map_or(0, |s| s.time);
        let (y, m, d, hh, mm, ss) = local_parts(start, self.offset);
        format!("performance-{y:04}{m:02}{d:02}-{hh:02}{mm:02}{ss:02}-{}.{}", self.kind.key(), self.format.key())
    }

    /// The number of data rows.
    pub fn rows(&self) -> usize {
        match self.kind {
            Kind::Table => self.table.rows.len(),
            Kind::System => self.range.len(),
            Kind::Process => self.range.iter().map(|s| s.procs.iter().filter(|p| self.matches(&p.id.stat.name, p.id.pid)).count()).sum(),
        }
    }

    /// The file size, roughly: the heading and the first rows, scaled to all rows.
    pub fn estimate(&self) -> u64 {
        const SAMPLE_ROWS: usize = 200;
        let mut head = Count(0);
        let _ = self.write_rows(&mut head, 0);
        let mut some = Count(0);
        let written = self.write_rows(&mut some, SAMPLE_ROWS).unwrap_or(0);
        let body = some.0.saturating_sub(head.0);
        let total = self.rows();
        if written == 0 {
            head.0
        } else {
            head.0 + (body as f64 / written as f64 * total as f64) as u64
        }
    }

    /// Writes the whole file.
    pub fn write(&self, w: &mut dyn Write) -> io::Result<()> {
        self.write_rows(w, usize::MAX).map(|_| ())
    }

    /// Writes the heading and at most `limit` rows; returns the rows written.
    fn write_rows(&self, w: &mut dyn Write, limit: usize) -> io::Result<usize> {
        let mut out = Out { w, format: self.format, encoding: self.encoding };
        if self.encoding == Encoding::Utf8Bom {
            out.w.write_all(b"\xEF\xBB\xBF")?;
        }
        match self.kind {
            Kind::Table => {
                out.row(&self.table.head)?;
                let mut n = 0;
                for row in self.table.rows.iter().take(limit) {
                    out.row(row)?;
                    n += 1;
                }
                Ok(n)
            }
            Kind::Process => self.process(&mut out, limit),
            Kind::System => self.system(&mut out, limit),
        }
    }

    fn process(&self, out: &mut Out, limit: usize) -> io::Result<usize> {
        const HEAD: [&str; 30] = [
            "time",
            "name",
            "pid",
            "cpu_ratio",
            "cpu_time_seconds",
            "cycles",
            "memory_bytes",
            "private_bytes",
            "working_set_bytes",
            "commit_bytes",
            "paged_pool_bytes",
            "nonpaged_pool_bytes",
            "page_faults",
            "page_faults_per_second",
            "disk_bytes_per_second",
            "io_read_bytes",
            "io_write_bytes",
            "io_other_bytes",
            "io_read_count",
            "io_write_count",
            "io_other_count",
            "network_bytes_per_second",
            "gpu_ratio",
            "gpu_memory_bytes",
            "gpu_shared_bytes",
            "handles",
            "threads",
            "user_objects",
            "gdi_objects",
            "not_responding",
        ];
        out.row(&HEAD.map(String::from))?;
        let mut n = 0;
        for sample in &self.range {
            let time = iso(sample.time, self.offset);
            let mut procs: Vec<&PRec> = sample.procs.iter().filter(|p| self.matches(&p.id.stat.name, p.id.pid)).collect();
            procs.sort_by_key(|p| p.id.pid);
            for p in procs {
                if n >= limit {
                    return Ok(n);
                }
                let kb = |v: u32| (v as u64 * 1024).to_string();
                let row = [
                    time.clone(),
                    p.id.stat.name.clone(),
                    p.id.pid.to_string(),
                    number(p.cpu as f64 / 10_000.0, 6),
                    number(p.cpu_cs as f64 / 100.0, 2),
                    p.cycles.to_string(),
                    kb(p.active_kb),
                    kb(p.private_kb),
                    kb(p.ws_kb),
                    kb(p.commit_kb),
                    kb(p.paged_kb),
                    kb(p.nonpaged_kb),
                    p.page_faults.to_string(),
                    number(p.pf_delta as f64 / sample.secs as f64, 2),
                    kb(p.disk_kbps),
                    p.io_read_bytes.to_string(),
                    p.io_write_bytes.to_string(),
                    p.io_other_bytes.to_string(),
                    p.io_reads.to_string(),
                    p.io_writes.to_string(),
                    p.io_other.to_string(),
                    p.net_bps.to_string(),
                    number(p.gpu as f64 / 10_000.0, 6),
                    kb(p.gpu_ded_kb),
                    kb(p.gpu_shared_kb),
                    p.handles.to_string(),
                    p.threads.to_string(),
                    p.user_objects.to_string(),
                    p.gdi_objects.to_string(),
                    boolean(p.status() == Status::NotResponding),
                ];
                out.row(&row)?;
                n += 1;
            }
        }
        Ok(n)
    }

    fn system(&self, out: &mut Out, limit: usize) -> io::Result<usize> {
        let mut head: Vec<String> =
            ["time", "cpu_ratio", "cpu_kernel_ratio", "cpu_clock_hz", "memory_used_bytes", "memory_total_bytes", "commit_bytes", "commit_limit_bytes"]
                .map(String::from)
                .to_vec();
        head.extend(self.disks.iter().map(|(n, _)| format!("disk{n}_active_ratio")));
        for (_, n, _) in &self.adapters {
            head.push(format!("gpu{n}_ratio"));
            head.push(format!("gpu{n}_memory_bytes"));
            head.push(format!("gpu{n}_memory_total_bytes"));
        }
        head.extend(["handles", "page_faults_per_second", "not_responding_processes"].map(String::from));
        out.row(&head)?;
        let mut n = 0;
        for s in &self.range {
            if n >= limit {
                break;
            }
            let mut row = vec![
                iso(s.time, self.offset),
                number(s.cpu as f64 / 100.0, 6),
                number(s.kernel as f64 / 100.0, 6),
                if s.mhz > 0.0 { number(s.mhz as f64 * 1e6, 0) } else { String::new() },
                s.mem_used.to_string(),
                s.mem_total.to_string(),
                s.commit.to_string(),
                s.commit_limit.to_string(),
            ];
            for (_, i) in &self.disks {
                row.push(number(s.disks.get(*i).copied().unwrap_or(f32::NAN) as f64 / 100.0, 6));
            }
            for (i, _, total) in &self.adapters {
                let present = s.gpus.get(*i).is_some_and(|v| !v.is_nan());
                row.push(number(s.gpus.get(*i).copied().unwrap_or(f32::NAN) as f64 / 100.0, 6));
                row.push(if present { s.gpu_mem.get(*i).copied().unwrap_or(0).to_string() } else { String::new() });
                row.push(if *total > 0 { total.to_string() } else { String::new() });
            }
            let handles: u64 = s.procs.iter().map(|p| p.handles as u64).sum();
            let faults: f64 = s.procs.iter().map(|p| p.pf_delta as f64).sum::<f64>() / s.secs as f64;
            let hung = s.procs.iter().filter(|p| p.status() == Status::NotResponding).count();
            row.push(handles.to_string());
            row.push(number(faults, 2));
            row.push(hung.to_string());
            out.row(&row)?;
            n += 1;
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_times() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20769), (2026, 11, 12));
        assert_eq!(civil_from_days(11016), (2000, 2, 29));
        // 2026-11-14 04:15:30 JST is 2026-11-13 19:15:30 UTC.
        let utc = (20770 * 86400 + 19 * 3600 + 15 * 60 + 30 + UNIX_FROM_1601) * SEC;
        assert_eq!(iso(utc, 9 * 3600), "2026-11-14T04:15:30+09:00");
        assert_eq!(iso(utc, -5 * 3600), "2026-11-13T14:15:30-05:00");
    }

    #[test]
    fn plain_numbers() {
        assert_eq!(number(0.042_000_1, 6), "0.042");
        assert_eq!(number(1234.0, 0), "1234");
        assert_eq!(number(-0.000_000_1, 6), "0");
        assert_eq!(number(f64::NAN, 2), "");
    }

    #[test]
    fn csv_quotes_and_tsv_flattens() {
        let mut buf = Vec::new();
        let mut out = Out { w: &mut buf, format: Format::Csv, encoding: Encoding::Utf8Bom };
        out.row(&["a,b".into(), "say \"hi\"".into(), "x".into()]).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "\"a,b\",\"say \"\"hi\"\"\",x\r\n");
        let mut buf = Vec::new();
        let mut out = Out { w: &mut buf, format: Format::Tsv, encoding: Encoding::Utf8Bom };
        out.row(&["a\tb".into(), "c".into()]).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "a b\tc\r\n");
    }

    fn job(kind: Kind) -> Job {
        use crate::pages::details::timeline::tests::{rec, sample};
        // 2026-11-14 04:15:30 JST, then one second later.
        let t0 = (20770 * 86400 + 19 * 3600 + 15 * 60 + 30 + UNIX_FROM_1601) * SEC;
        let mut hung = rec(1, 30.0, 0);
        hung.flags = 2;
        Job {
            kind,
            format: Format::Csv,
            encoding: Encoding::Utf8Bom,
            range: vec![sample(t0, vec![rec(1, 10.0, 150), rec(2, 4.0, 0)]), sample(t0 + SEC, vec![hung])],
            table: Table { head: vec!["name".into(), "cpu_max_ratio".into()], rows: vec![vec!["a,b".into(), "0.3".into()]] },
            terms: Vec::new(),
            offset: 9 * 3600,
            disks: vec![(0, 0)],
            adapters: vec![(0, 0, 12 << 30)],
        }
    }

    fn text_of(j: &Job) -> String {
        let mut buf = Vec::new();
        j.write(&mut buf).unwrap();
        assert!(buf.starts_with(b"\xEF\xBB\xBF"));
        String::from_utf8(buf[3..].to_vec()).unwrap()
    }

    #[test]
    fn table_file_is_the_table() {
        let j = job(Kind::Table);
        assert_eq!(text_of(&j), "name,cpu_max_ratio\r\n\"a,b\",0.3\r\n");
        assert_eq!(j.file_name(), "performance-20261114-041530-table.csv");
    }

    #[test]
    fn process_and_system_files() {
        let mut j = job(Kind::Process);
        j.terms = vec!["p1".into()];
        let text = text_of(&j);
        let lines: Vec<&str> = text.split("\r\n").filter(|l| !l.is_empty()).collect();
        assert!(lines[0].starts_with("time,name,pid,cpu_ratio,cpu_time_seconds,"));
        assert_eq!(lines.len(), 3, "only p1, at each sample");
        assert!(lines[1].starts_with("2026-11-14T04:15:30+09:00,p1,1,0.1,1.5,"));
        assert!(lines[2].ends_with(",true"));
        assert_eq!(j.rows(), 2);

        let j = job(Kind::System);
        let text = text_of(&j);
        let lines: Vec<&str> = text.split("\r\n").collect();
        assert_eq!(
            lines[0],
            "time,cpu_ratio,cpu_kernel_ratio,cpu_clock_hz,memory_used_bytes,memory_total_bytes,commit_bytes,commit_limit_bytes,\
             disk0_active_ratio,gpu0_ratio,gpu0_memory_bytes,gpu0_memory_total_bytes,handles,page_faults_per_second,not_responding_processes"
        );
        assert!(lines[1].starts_with("2026-11-14T04:15:30+09:00,0.25,0.05,4000000000,8589934592,17179869184,"));
        assert!(lines[2].ends_with(",1"));
    }

    #[test]
    fn estimate_is_close() {
        let j = job(Kind::Process);
        let mut buf = Vec::new();
        j.write(&mut buf).unwrap();
        assert_eq!(j.estimate(), buf.len() as u64);
    }

    #[test]
    fn shift_jis_replaces_what_it_lacks() {
        assert_eq!(shift_jis("あA"), vec![0x82, 0xA0, b'A']);
        // An emoji has no Shift_JIS form.
        assert!(shift_jis("😀").iter().all(|b| *b == b'?'));
    }
}
