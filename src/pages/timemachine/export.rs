//! Writing the time machine's samples to a file: the table as shown, the values of each
//! process at each sample, or the machine-wide values at each sample.
//!
//! Headings are snake_case with the unit at the end (`_bytes`, `_ratio`, `_seconds`,
//! `_per_second`, `_hz`); values are plain numbers (ratios from 0 to 1, sizes in bytes);
//! times are ISO 8601 with the offset. Paths, command lines and user names are never written.

use super::{range_stats, row_id, Sample, Target, SEC};
use crate::i18n::tr;
use std::collections::HashMap;
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

/// Everything a file needs, taken from the page when export starts, so the file can be
/// written after the save dialog without the page.
pub struct Job {
    pub kind: Kind,
    pub format: Format,
    pub encoding: Encoding,
    pub targets: Vec<Target>,
    /// The selected samples (one for a single time).
    pub range: Vec<Arc<Sample>>,
    /// Table rows in the order shown (filtered), by row id. None when the table on screen
    /// is not what is written (the whole record): then the filter applies here, and the rows
    /// go heaviest first by the first target's maximum, as the table starts.
    pub order: Option<Vec<u64>>,
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
fn number(v: f64, decimals: usize) -> String {
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

impl Target {
    /// The column name: the item, then the statistic if any, then the unit.
    fn column(self, stat: Option<&str>) -> String {
        let (base, unit) = match self {
            Target::Cpu => ("cpu", "_ratio"),
            Target::Mem => ("memory", "_bytes"),
            Target::Commit => ("commit", "_bytes"),
            Target::Disk => ("disk", "_bytes_per_second"),
            Target::Gpu => ("gpu", "_ratio"),
            Target::GpuMem => ("gpu_memory", "_bytes"),
            Target::Handles => ("handles", ""),
            Target::Faults => ("page_faults", "_per_second"),
            Target::CpuDetail | Target::Hung => ("", ""),
        };
        match stat {
            Some(st) => format!("{base}_{st}{unit}"),
            None => format!("{base}{unit}"),
        }
    }

    fn places(self) -> usize {
        match self {
            Target::Cpu | Target::Gpu => 6,
            Target::Mem | Target::Commit | Target::GpuMem => 0,
            _ => 2,
        }
    }

    fn cell(self, v: f64) -> String {
        number(v, self.places())
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
    fn values(&self) -> Vec<Target> {
        self.targets.iter().copied().filter(|t| t.per_process()).collect()
    }

    fn hung(&self) -> bool {
        self.targets.contains(&Target::Hung)
    }

    fn single(&self) -> bool {
        self.range.len() == 1
    }

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
            Kind::Table => match &self.order {
                Some(o) => o.len(),
                None if self.single() => self.range[0].procs.iter().filter(|p| self.matches(&p.stat.name, p.pid)).count(),
                None => range_stats(&self.range, &self.values()).iter().filter(|s| self.matches(&s.stat.name, s.pid)).count(),
            },
            Kind::System => self.range.len(),
            Kind::Process => self.range.iter().map(|s| s.procs.iter().filter(|p| self.matches(&p.stat.name, p.pid)).count()).sum(),
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
            Kind::Table if self.single() => self.table_single(&mut out, limit),
            Kind::Table => self.table_range(&mut out, limit),
            Kind::Process => self.process(&mut out, limit),
            Kind::System => self.system(&mut out, limit),
        }
    }

    fn table_single(&self, out: &mut Out, limit: usize) -> io::Result<usize> {
        let values = self.values();
        let mut head = vec!["name".to_string(), "pid".to_string()];
        head.extend(values.iter().map(|t| t.column(None)));
        if self.hung() {
            head.push("not_responding".into());
        }
        out.row(&head)?;
        let Some(sample) = self.range.first() else { return Ok(0) };
        let procs: Vec<&super::TmProc> = match &self.order {
            Some(order) => {
                let by_id: HashMap<u64, &super::TmProc> = sample.procs.iter().map(|p| (row_id(p.pid, p.create_time), p)).collect();
                order.iter().filter_map(|id| by_id.get(id).copied()).collect()
            }
            None => {
                let mut v: Vec<&super::TmProc> = sample.procs.iter().filter(|p| self.matches(&p.stat.name, p.pid)).collect();
                match values.first() {
                    Some(t) => v.sort_by(|a, b| t.raw(b).total_cmp(&t.raw(a)).then(a.pid.cmp(&b.pid))),
                    None => v.sort_by_key(|a| a.stat.name.to_lowercase()),
                }
                v
            }
        };
        let mut n = 0;
        for p in procs {
            if n >= limit {
                break;
            }
            let mut row = vec![p.stat.name.clone(), p.pid.to_string()];
            row.extend(values.iter().map(|t| t.cell(t.raw(p))));
            if self.hung() {
                row.push(boolean(p.hung));
            }
            out.row(&row)?;
            n += 1;
        }
        Ok(n)
    }

    fn table_range(&self, out: &mut Out, limit: usize) -> io::Result<usize> {
        let values = self.values();
        let mut head: Vec<String> = ["name", "pid", "born", "died", "born_at", "died_at"].iter().map(|s| s.to_string()).collect();
        for t in &values {
            for st in ["start", "end", "delta", "avg", "max"] {
                head.push(t.column(Some(st)));
            }
        }
        if self.hung() {
            head.push("not_responding_seconds".into());
        }
        out.row(&head)?;
        if limit == 0 {
            return Ok(0);
        }
        let stats = range_stats(&self.range, &values);
        let rows: Vec<&super::RangeStat> = match &self.order {
            Some(order) => {
                let by_id: HashMap<u64, &super::RangeStat> = stats.iter().map(|s| (row_id(s.pid, s.create_time), s)).collect();
                order.iter().filter_map(|id| by_id.get(id).copied()).collect()
            }
            None => {
                let mut v: Vec<&super::RangeStat> = stats.iter().filter(|s| self.matches(&s.stat.name, s.pid)).collect();
                if values.is_empty() {
                    v.sort_by_key(|a| a.stat.name.to_lowercase());
                } else {
                    v.sort_by(|a, b| b.max[0].total_cmp(&a.max[0]).then(a.pid.cmp(&b.pid)));
                }
                v
            }
        };
        let mut n = 0;
        for s in rows {
            if n >= limit {
                break;
            }
            let when = |born_or_died: bool, t: i64| if born_or_died { iso(t, self.offset) } else { String::new() };
            let mut row = vec![
                s.stat.name.clone(),
                s.pid.to_string(),
                boolean(s.born),
                boolean(s.died),
                when(s.born, s.born_at),
                when(s.died, s.died_at),
            ];
            for (j, t) in values.iter().enumerate() {
                row.push(t.cell(s.start[j]));
                row.push(t.cell(s.end[j]));
                row.push(t.cell(s.end[j] - s.start[j]));
                row.push(t.cell(s.avg(j)));
                row.push(t.cell(s.max[j]));
            }
            if self.hung() {
                row.push(number(s.hung as f64 / SEC as f64, 1));
            }
            out.row(&row)?;
            n += 1;
        }
        Ok(n)
    }

    fn process(&self, out: &mut Out, limit: usize) -> io::Result<usize> {
        let values = self.values();
        let mut head = vec!["time".to_string(), "name".to_string(), "pid".to_string()];
        head.extend(values.iter().map(|t| t.column(None)));
        if self.hung() {
            head.push("not_responding".into());
        }
        out.row(&head)?;
        let mut n = 0;
        for sample in &self.range {
            let time = iso(sample.time, self.offset);
            let mut procs: Vec<&super::TmProc> = sample.procs.iter().filter(|p| self.matches(&p.stat.name, p.pid)).collect();
            procs.sort_by_key(|p| p.pid);
            for p in procs {
                if n >= limit {
                    return Ok(n);
                }
                let mut row = vec![time.clone(), p.stat.name.clone(), p.pid.to_string()];
                row.extend(values.iter().map(|t| t.cell(t.raw(p))));
                if self.hung() {
                    row.push(boolean(p.hung));
                }
                out.row(&row)?;
                n += 1;
            }
        }
        Ok(n)
    }

    fn system(&self, out: &mut Out, limit: usize) -> io::Result<usize> {
        let mut head = vec!["time".to_string()];
        for t in &self.targets {
            match t {
                Target::Cpu => head.push("cpu_ratio".into()),
                Target::CpuDetail => head.extend(["cpu_kernel_ratio".to_string(), "cpu_clock_hz".to_string()]),
                Target::Mem => head.extend(["memory_used_bytes".to_string(), "memory_total_bytes".to_string()]),
                Target::Commit => head.extend(["commit_bytes".to_string(), "commit_limit_bytes".to_string()]),
                Target::Disk => head.extend(self.disks.iter().map(|(n, _)| format!("disk{n}_active_ratio"))),
                Target::Gpu => head.extend(self.adapters.iter().map(|(_, n, _)| format!("gpu{n}_ratio"))),
                Target::GpuMem => {
                    for (_, n, _) in &self.adapters {
                        head.push(format!("gpu{n}_memory_bytes"));
                        head.push(format!("gpu{n}_memory_total_bytes"));
                    }
                }
                Target::Handles => head.push("handles".into()),
                Target::Faults => head.push("page_faults_per_second".into()),
                Target::Hung => head.push("not_responding_processes".into()),
            }
        }
        out.row(&head)?;
        let mut n = 0;
        for s in &self.range {
            if n >= limit {
                break;
            }
            let mut row = vec![iso(s.time, self.offset)];
            for t in &self.targets {
                match t {
                    Target::Cpu => row.push(number(s.cpu as f64 / 100.0, 6)),
                    Target::CpuDetail => {
                        row.push(number(s.kernel as f64 / 100.0, 6));
                        row.push(if s.mhz > 0.0 { number(s.mhz as f64 * 1e6, 0) } else { String::new() });
                    }
                    Target::Mem => {
                        row.push(s.mem_used.to_string());
                        row.push(s.mem_total.to_string());
                    }
                    Target::Commit => {
                        row.push(s.commit.to_string());
                        row.push(s.commit_limit.to_string());
                    }
                    Target::Disk => {
                        for (_, i) in &self.disks {
                            row.push(number(s.disks.get(*i).copied().unwrap_or(f32::NAN) as f64 / 100.0, 6));
                        }
                    }
                    Target::Gpu => {
                        for (i, _, _) in &self.adapters {
                            row.push(number(s.gpus.get(*i).copied().unwrap_or(f32::NAN) as f64 / 100.0, 6));
                        }
                    }
                    Target::GpuMem => {
                        for (i, _, total) in &self.adapters {
                            let present = s.gpus.get(*i).is_some_and(|v| !v.is_nan());
                            row.push(if present { s.gpu_mem.get(*i).copied().unwrap_or(0).to_string() } else { String::new() });
                            row.push(if *total > 0 { total.to_string() } else { String::new() });
                        }
                    }
                    Target::Handles => row.push(s.handles.to_string()),
                    Target::Faults => row.push(number(s.faults as f64, 2)),
                    Target::Hung => row.push(s.procs.iter().filter(|p| p.hung).count().to_string()),
                }
            }
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

    fn proc(pid: u32, name: &str, cpu: f32, hung: bool) -> super::super::TmProc {
        super::super::TmProc {
            stat: Arc::new(crate::system::model::ProcStatic {
                name: name.into(),
                user: Arc::from("someone"),
                arch: "x64",
                elevated: None,
                cmdline: "secret --token".into(),
                image: None,
                package: None,
                platform: 64,
                system_dir: false,
                protect: crate::system::model::Protect::default(),
            }),
            create_time: pid as i64 * SEC,
            pid,
            cpu,
            mem_kb: 2048,
            gpu: 0.0,
            disk_bps: 1000.0,
            commit_kb: 4096,
            handles: 10,
            faults: 0.5,
            gpu_mem_kb: 0,
            hung,
        }
    }

    fn sample(time: i64, procs: Vec<super::super::TmProc>) -> Arc<Sample> {
        Arc::new(Sample {
            time,
            cpu: 25.0,
            kernel: 5.0,
            mhz: 4000.0,
            mem_pct: 50.0,
            mem_used: 8 << 30,
            mem_total: 16 << 30,
            commit: 10 << 30,
            commit_limit: 20 << 30,
            disks: vec![12.5].into_boxed_slice(),
            gpus: vec![40.0].into_boxed_slice(),
            gpu_mem: vec![1 << 30].into_boxed_slice(),
            handles: 1234,
            faults: 7.5,
            procs: procs.into_boxed_slice(),
        })
    }

    fn job(kind: Kind, targets: Vec<Target>, order: Vec<u64>) -> Job {
        // 2026-11-14 04:15:30 JST, then one and two seconds later.
        let t0 = (20770 * 86400 + 19 * 3600 + 15 * 60 + 30 + UNIX_FROM_1601) * SEC;
        Job {
            kind,
            format: Format::Csv,
            encoding: Encoding::Utf8Bom,
            targets,
            range: vec![
                sample(t0, vec![proc(1, "a.exe", 10.0, false), proc(2, "b,c.exe", 4.0, false)]),
                sample(t0 + SEC, vec![proc(1, "a.exe", 30.0, true)]),
                sample(t0 + 2 * SEC, vec![proc(1, "a.exe", 20.0, true), proc(3, "d.exe", 2.0, false)]),
            ],
            order: Some(order),
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
    fn table_range_file() {
        let order = vec![row_id(1, SEC), row_id(2, 2 * SEC), row_id(3, 3 * SEC)];
        let j = job(Kind::Table, vec![Target::Cpu, Target::Mem, Target::Hung], order);
        let text = text_of(&j);
        let lines: Vec<&str> = text.split("\r\n").collect();
        assert_eq!(
            lines[0],
            "name,pid,born,died,born_at,died_at,cpu_start_ratio,cpu_end_ratio,cpu_delta_ratio,cpu_avg_ratio,cpu_max_ratio,\
             memory_start_bytes,memory_end_bytes,memory_delta_bytes,memory_avg_bytes,memory_max_bytes,not_responding_seconds"
        );
        assert_eq!(lines[1], "a.exe,1,false,false,,,0.1,0.2,0.1,0.2,0.3,2097152,2097152,0,2097152,2097152,2");
        // A comma in a name is quoted; a process that died has its time.
        assert_eq!(lines[2], "\"b,c.exe\",2,false,true,,2026-11-14T04:15:31+09:00,0.04,0.04,0,0.04,0.04,2097152,2097152,0,2097152,2097152,0");
        assert!(lines[3].starts_with("d.exe,3,true,false,1601-01-01T09:00:03+09:00,,"));
        // No user name, path or command line.
        assert!(!text.contains("someone") && !text.contains("secret"));
        assert_eq!(j.file_name(), "performance-20261114-041530-table.csv");
    }

    #[test]
    fn process_and_system_files() {
        let mut j = job(Kind::Process, vec![Target::Cpu, Target::Hung], Vec::new());
        j.terms = vec!["a.exe".into()];
        let text = text_of(&j);
        let lines: Vec<&str> = text.split("\r\n").filter(|l| !l.is_empty()).collect();
        assert_eq!(lines[0], "time,name,pid,cpu_ratio,not_responding");
        assert_eq!(lines.len(), 4, "only a.exe, at each sample");
        assert_eq!(lines[2], "2026-11-14T04:15:31+09:00,a.exe,1,0.3,true");
        assert_eq!(j.rows(), 3);

        let j = job(Kind::System, vec![Target::Cpu, Target::CpuDetail, Target::Mem, Target::Disk, Target::GpuMem, Target::Hung], Vec::new());
        let text = text_of(&j);
        let lines: Vec<&str> = text.split("\r\n").collect();
        assert_eq!(
            lines[0],
            "time,cpu_ratio,cpu_kernel_ratio,cpu_clock_hz,memory_used_bytes,memory_total_bytes,disk0_active_ratio,\
             gpu0_memory_bytes,gpu0_memory_total_bytes,not_responding_processes"
        );
        assert_eq!(lines[1], "2026-11-14T04:15:30+09:00,0.25,0.05,4000000000,8589934592,17179869184,0.125,1073741824,12884901888,0");
        assert_eq!(lines[2].rsplit(',').next(), Some("1"));
    }

    #[test]
    fn whole_record_table_has_its_own_order_and_filter() {
        let mut j = job(Kind::Table, vec![Target::Cpu], Vec::new());
        j.order = None;
        let text = text_of(&j);
        let names: Vec<&str> = text.split("\r\n").skip(1).filter(|l| !l.is_empty()).map(|l| l.split(',').next().unwrap()).collect();
        // Heaviest first by the CPU maximum: a (0.3), b,c (0.04), d (0.02).
        assert_eq!(names, ["a.exe", "\"b", "d.exe"]);
        j.terms = vec!["d.".into()];
        assert_eq!(j.rows(), 1);
    }

    #[test]
    fn estimate_is_close() {
        let j = job(Kind::Process, vec![Target::Cpu, Target::Mem], Vec::new());
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
