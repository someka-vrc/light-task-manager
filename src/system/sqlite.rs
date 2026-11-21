//! A minimal SQLite binding over the `winsqlite3.dll` that ships with Windows 10 and later.
//!
//! The library is loaded when first needed, so the EXE neither carries SQLite nor stops
//! starting where the DLL is missing: `api()` is then `None` and recording stays off.

use crate::system::win::wide;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::Path;
use std::sync::OnceLock;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW};

const LOAD_LIBRARY_SEARCH_SYSTEM32: u32 = 0x800;
const OK: c_int = 0;
const ROW: c_int = 100;
const DONE: c_int = 101;
const OPEN_READWRITE: c_int = 0x2;
const OPEN_CREATE: c_int = 0x4;
/// `SQLITE_TRANSIENT`: SQLite copies the bound text at once.
const TRANSIENT: isize = -1;

type Open = unsafe extern "C" fn(*const c_char, *mut *mut c_void, c_int, *const c_char) -> c_int;
type Close = unsafe extern "C" fn(*mut c_void) -> c_int;
type Exec = unsafe extern "C" fn(*mut c_void, *const c_char, *const c_void, *mut c_void, *mut *mut c_char) -> c_int;
type Prepare = unsafe extern "C" fn(*mut c_void, *const c_char, c_int, *mut *mut c_void, *mut *const c_char) -> c_int;
type Step = unsafe extern "C" fn(*mut c_void) -> c_int;
type BindI64 = unsafe extern "C" fn(*mut c_void, c_int, i64) -> c_int;
type BindF64 = unsafe extern "C" fn(*mut c_void, c_int, f64) -> c_int;
type BindText = unsafe extern "C" fn(*mut c_void, c_int, *const c_char, c_int, isize) -> c_int;
type BindNull = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;
#[cfg(test)]
type ColumnI64 = unsafe extern "C" fn(*mut c_void, c_int) -> i64;
type ErrMsg = unsafe extern "C" fn(*mut c_void) -> *const c_char;
type BusyTimeout = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;

pub struct Api {
    open: Open,
    close: Close,
    exec: Exec,
    prepare: Prepare,
    step: Step,
    reset: Step,
    finalize: Step,
    bind_i64: BindI64,
    bind_f64: BindF64,
    bind_text: BindText,
    bind_null: BindNull,
    #[cfg(test)]
    column_i64: ColumnI64,
    errmsg: ErrMsg,
    busy_timeout: BusyTimeout,
}

fn load() -> Option<Api> {
    let name = wide("winsqlite3.dll");
    let lib = unsafe { LoadLibraryExW(name.as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32) };
    if lib.is_null() {
        return None;
    }
    macro_rules! sym {
        ($name:literal) => {{
            let p = unsafe { GetProcAddress(lib, concat!($name, "\0").as_ptr()) }?;
            unsafe { std::mem::transmute_copy(&p) }
        }};
    }
    Some(Api {
        open: sym!("sqlite3_open_v2"),
        close: sym!("sqlite3_close"),
        exec: sym!("sqlite3_exec"),
        prepare: sym!("sqlite3_prepare_v2"),
        step: sym!("sqlite3_step"),
        reset: sym!("sqlite3_reset"),
        finalize: sym!("sqlite3_finalize"),
        bind_i64: sym!("sqlite3_bind_int64"),
        bind_f64: sym!("sqlite3_bind_double"),
        bind_text: sym!("sqlite3_bind_text"),
        bind_null: sym!("sqlite3_bind_null"),
        #[cfg(test)]
        column_i64: sym!("sqlite3_column_int64"),
        errmsg: sym!("sqlite3_errmsg"),
        busy_timeout: sym!("sqlite3_busy_timeout"),
    })
}

/// The loaded library, or None where it is not there.
pub fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(load).as_ref()
}

pub struct Db {
    h: *mut c_void,
    api: &'static Api,
}

// A connection is used by one thread at a time.
unsafe impl Send for Db {}

impl Db {
    pub fn open(path: &Path) -> Result<Db, String> {
        let api = api().ok_or("winsqlite3.dll is not available")?;
        let file = CString::new(path.to_str().ok_or("the path is not valid text")?).map_err(|e| e.to_string())?;
        let mut h = std::ptr::null_mut();
        let rc = unsafe { (api.open)(file.as_ptr(), &mut h, OPEN_READWRITE | OPEN_CREATE, std::ptr::null()) };
        let db = Db { h, api };
        if rc != OK {
            let msg = db.error();
            return Err(format!("open: {msg}"));
        }
        unsafe { (api.busy_timeout)(h, 2000) };
        Ok(db)
    }

    fn error(&self) -> String {
        if self.h.is_null() {
            return "unknown".into();
        }
        let p = unsafe { (self.api.errmsg)(self.h) };
        if p.is_null() {
            return "unknown".into();
        }
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }

    /// Runs statements that return nothing we need.
    pub fn exec(&self, sql: &str) -> Result<(), String> {
        let sql = CString::new(sql).map_err(|e| e.to_string())?;
        let rc = unsafe { (self.api.exec)(self.h, sql.as_ptr(), std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut()) };
        if rc == OK {
            Ok(())
        } else {
            Err(self.error())
        }
    }

    pub fn prepare(&self, sql: &str) -> Result<Stmt, String> {
        let sql = CString::new(sql).map_err(|e| e.to_string())?;
        let mut h = std::ptr::null_mut();
        let rc = unsafe { (self.api.prepare)(self.h, sql.as_ptr(), -1, &mut h, std::ptr::null_mut()) };
        if rc != OK || h.is_null() {
            return Err(self.error());
        }
        Ok(Stmt { h, api: self.api })
    }

    /// The first column of the first row, when there is one.
    #[cfg(test)]
    pub fn query_i64(&self, sql: &str) -> Option<i64> {
        let stmt = self.prepare(sql).ok()?;
        match stmt.step() {
            Ok(true) => Some(stmt.column_i64(0)),
            _ => None,
        }
    }

}

impl Drop for Db {
    fn drop(&mut self) {
        if !self.h.is_null() {
            unsafe { (self.api.close)(self.h) };
        }
    }
}

pub struct Stmt {
    h: *mut c_void,
    api: &'static Api,
}

unsafe impl Send for Stmt {}

impl Stmt {
    pub fn bind_i64(&self, i: i32, v: i64) {
        unsafe { (self.api.bind_i64)(self.h, i, v) };
    }

    pub fn bind_f64(&self, i: i32, v: f64) {
        unsafe { (self.api.bind_f64)(self.h, i, v) };
    }

    pub fn bind_text(&self, i: i32, v: &str) {
        unsafe { (self.api.bind_text)(self.h, i, v.as_ptr().cast(), v.len() as c_int, TRANSIENT) };
    }

    pub fn bind_null(&self, i: i32) {
        unsafe { (self.api.bind_null)(self.h, i) };
    }

    pub fn bind_opt_f64(&self, i: i32, v: Option<f64>) {
        match v {
            Some(v) => self.bind_f64(i, v),
            None => self.bind_null(i),
        }
    }

    pub fn bind_opt_i64(&self, i: i32, v: Option<i64>) {
        match v {
            Some(v) => self.bind_i64(i, v),
            None => self.bind_null(i),
        }
    }

    pub fn bind_opt_text(&self, i: i32, v: Option<&str>) {
        match v {
            Some(v) => self.bind_text(i, v),
            None => self.bind_null(i),
        }
    }

    /// Ok(true) when a row is there, Ok(false) when the statement is done.
    pub fn step(&self) -> Result<bool, i32> {
        match unsafe { (self.api.step)(self.h) } {
            ROW => Ok(true),
            DONE => Ok(false),
            rc => Err(rc),
        }
    }

    /// Makes the statement ready to run again. Bindings stay until they are set anew.
    pub fn reset(&self) {
        unsafe { (self.api.reset)(self.h) };
    }

    #[cfg(test)]
    pub fn column_i64(&self, i: i32) -> i64 {
        unsafe { (self.api.column_i64)(self.h, i) }
    }
}

impl Drop for Stmt {
    fn drop(&mut self) {
        unsafe { (self.api.finalize)(self.h) };
    }
}
