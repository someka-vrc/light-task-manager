//! Startup tasks of packaged (MSIX / Store) apps: the `windows.startupTask`
//! extensions in each package's AppxManifest.xml, with the state Windows keeps
//! under AppModel\SystemAppData (what Settings > Apps > Startup changes).

use crate::system::win::{from_wide, wide};
use std::collections::HashMap;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::UI::Shell::SHLoadIndirectString;

const APPMODEL: &str = "Software\\Classes\\Local Settings\\Software\\Microsoft\\Windows\\CurrentVersion\\AppModel\\";

// StartupTaskState.
const DISABLED_BY_USER: u32 = 1;
const ENABLED: u32 = 2;
const DISABLED_BY_POLICY: u32 = 3;
const ENABLED_BY_POLICY: u32 = 4;

pub struct Task {
    pub name: String,
    pub publisher: String,
    /// Full path of the executable the task starts.
    pub exe: String,
    /// `<package family name>\<task id>`, unique and stable across updates.
    pub key: String,
    pub enabled: bool,
}

/// The package full names registered for the current user (one per family, the newest).
fn packages() -> Vec<(String, String)> {
    let mut newest: HashMap<String, (Vec<u32>, String, String)> = HashMap::new();
    unsafe {
        let mut hk: HKEY = null_mut();
        let sub = wide(&format!("{APPMODEL}Repository\\Packages"));
        if RegOpenKeyExW(HKEY_CURRENT_USER, sub.as_ptr(), 0, KEY_READ, &mut hk) != 0 {
            return Vec::new();
        }
        let mut index = 0;
        loop {
            let mut name = vec![0u16; 512];
            let mut len = name.len() as u32;
            let r = RegEnumKeyExW(hk, index, name.as_mut_ptr(), &mut len, null(), null_mut(), null_mut(), null_mut());
            index += 1;
            if r == ERROR_NO_MORE_ITEMS {
                break;
            }
            if r != 0 {
                continue;
            }
            let full = String::from_utf16_lossy(&name[..len as usize]);
            // Name_Version_Architecture_ResourceId_PublisherId; resource packs and bundles carry a resource id.
            let parts: Vec<&str> = full.split('_').collect();
            if parts.len() != 5 || !parts[3].is_empty() {
                continue;
            }
            let Some(root) = string_value(hk, &full, "PackageRootFolder") else { continue };
            let version: Vec<u32> = parts[1].split('.').map(|v| v.parse().unwrap_or(0)).collect();
            let family = format!("{}_{}", parts[0], parts[4]);
            if newest.get(&family).is_none_or(|(v, _, _)| *v < version) {
                newest.insert(family, (version, full, root));
            }
        }
        RegCloseKey(hk);
    }
    newest.into_values().map(|(_, full, root)| (full, root)).collect()
}

fn string_value(root: HKEY, key: &str, value: &str) -> Option<String> {
    let (k, v) = (wide(key), wide(value));
    let mut buf = vec![0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(root, k.as_ptr(), v.as_ptr(), RRF_RT_REG_SZ, null_mut(), buf.as_mut_ptr().cast(), &mut size)
    };
    (r == 0).then(|| from_wide(&buf))
}

fn state(key: &str) -> Option<u32> {
    let (k, v) = (wide(&format!("{APPMODEL}SystemAppData\\{key}")), wide("State"));
    let mut data = 0u32;
    let mut size = 4u32;
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, k.as_ptr(), v.as_ptr(), RRF_RT_REG_DWORD, null_mut(), (&mut data as *mut u32).cast(), &mut size)
    };
    (r == 0).then_some(data)
}

/// Turns a task on or off the way Settings does (on = Enabled, off = DisabledByUser).
pub fn set_enabled(key: &str, enable: bool) -> Result<(), u32> {
    if matches!(state(key), Some(DISABLED_BY_POLICY | ENABLED_BY_POLICY)) {
        return Err(ERROR_ACCESS_DENIED);
    }
    let k = wide(&format!("{APPMODEL}SystemAppData\\{key}"));
    let v = wide("State");
    let data = if enable { ENABLED } else { DISABLED_BY_USER };
    unsafe {
        let mut hk: HKEY = null_mut();
        let r = RegCreateKeyExW(HKEY_CURRENT_USER, k.as_ptr(), 0, null(), 0, KEY_SET_VALUE, null(), &mut hk, null_mut());
        if r != 0 {
            return Err(r);
        }
        let r = RegSetValueExW(hk, v.as_ptr(), 0, REG_DWORD, (&data as *const u32).cast(), 4);
        RegCloseKey(hk);
        if r != 0 {
            return Err(r);
        }
    }
    Ok(())
}

/// The attributes of one start tag, entities decoded.
fn attrs(tag: &str) -> Vec<(&str, String)> {
    let mut out = Vec::new();
    let mut rest = tag;
    while let Some(eq) = rest.find('=') {
        let key = rest[..eq].trim();
        let after = rest[eq + 1..].trim_start();
        let Some(quote) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else { break };
        let Some(end) = after[1..].find(quote) else { break };
        out.push((key.rsplit(char::is_whitespace).next().unwrap_or(key), unescape(&after[1..1 + end])));
        rest = &after[end + 2..];
    }
    out
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

fn attr<'a>(a: &'a [(&str, String)], name: &str) -> Option<&'a str> {
    a.iter().find(|(k, _)| *k == name).map(|(_, v)| v.as_str())
}

/// The text of the first `<name>…</name>` element.
fn element_text(xml: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find('<')?;
    Some(unescape(xml[start..start + end].trim()))
}

/// Resolves an `ms-resource:` reference against the package's resources.
fn resource(text: &str, full_name: &str) -> String {
    let Some(rest) = text.strip_prefix("ms-resource:") else { return text.to_string() };
    let package = full_name.split('_').next().unwrap_or("");
    let uri = if rest.starts_with("//") {
        format!("ms-resource:{rest}")
    } else if rest.starts_with('/') {
        format!("ms-resource://{package}{rest}")
    } else if rest.contains('/') {
        format!("ms-resource://{package}/{rest}")
    } else {
        format!("ms-resource://{package}/resources/{rest}")
    };
    let src = wide(&format!("@{{{full_name}?{uri}}}"));
    let mut buf = vec![0u16; 512];
    let hr = unsafe { SHLoadIndirectString(src.as_ptr(), buf.as_mut_ptr(), buf.len() as u32, null()) };
    if hr < 0 {
        return String::new();
    }
    from_wide(&buf)
}

/// The startup tasks one manifest declares: (task id, enabled by default, display name, executable).
fn startup_tasks(xml: &str) -> Vec<(String, bool, String, String)> {
    let mut out = Vec::new();
    let mut app_exe = String::new();
    let mut ext_exe: Option<String> = None;
    let mut at = 0;
    while let Some(lt) = xml[at..].find('<') {
        let start = at + lt;
        if xml[start..].starts_with("<!--") {
            at = xml[start..].find("-->").map_or(xml.len(), |e| start + e + 3);
            continue;
        }
        let Some(gt) = xml[start..].find('>') else { break };
        let tag = &xml[start + 1..start + gt];
        at = start + gt + 1;
        if tag.starts_with(['?', '!']) {
            continue;
        }
        let closing = tag.starts_with('/');
        let name = tag.trim_start_matches('/').split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("");
        let local = name.rsplit(':').next().unwrap_or(name);
        match local {
            "Application" if !closing => app_exe = attr(&attrs(tag), "Executable").unwrap_or("").to_string(),
            "Extension" if closing => ext_exe = None,
            "Extension" => {
                let a = attrs(tag);
                if attr(&a, "Category") == Some("windows.startupTask") {
                    ext_exe = Some(attr(&a, "Executable").unwrap_or(&app_exe).to_string());
                }
            }
            "StartupTask" if !closing => {
                let (Some(exe), a) = (ext_exe.clone(), attrs(tag)) else { continue };
                let Some(id) = attr(&a, "TaskId") else { continue };
                let enabled = attr(&a, "Enabled").is_some_and(|v| v.eq_ignore_ascii_case("true"));
                let display = attr(&a, "DisplayName").unwrap_or("").to_string();
                out.push((id.to_string(), enabled, display, exe.replace('/', "\\")));
            }
            _ => {}
        }
    }
    out
}

pub fn tasks() -> Vec<Task> {
    let mut out = Vec::new();
    for (full, root) in packages() {
        // Packages with an external location keep their files there but the manifest in WindowsApps.
        let program_files = std::env::var("ProgramW6432").or_else(|_| std::env::var("ProgramFiles")).unwrap_or_default();
        let manifest = [format!("{root}\\AppxManifest.xml"), format!("{program_files}\\WindowsApps\\{full}\\AppxManifest.xml")]
            .into_iter()
            .find_map(|p| std::fs::read_to_string(p).ok());
        let Some(xml) = manifest else { continue };
        if !xml.contains("windows.startupTask") {
            continue;
        }
        let parts: Vec<&str> = full.split('_').collect();
        let family = format!("{}_{}", parts[0], parts[4]);
        let package_name = element_text(&xml, "DisplayName").map(|t| resource(&t, &full)).unwrap_or_default();
        let publisher = element_text(&xml, "PublisherDisplayName").map(|t| resource(&t, &full)).unwrap_or_default();
        for (id, default, display, exe) in startup_tasks(&xml) {
            let key = format!("{family}\\{id}");
            let st = state(&key);
            let name = Some(resource(&display, &full)).filter(|n| !n.is_empty()).unwrap_or_else(|| package_name.clone());
            out.push(Task {
                name: if name.is_empty() { parts[0].to_string() } else { name },
                publisher: publisher.clone(),
                exe: format!("{root}\\{exe}"),
                enabled: st.map_or(default, |s| s == ENABLED || s == ENABLED_BY_POLICY),
                key,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_startup_tasks() {
        let xml = r#"<?xml version="1.0"?>
<Package xmlns:uap5="x" xmlns:desktop="y">
  <!-- a <comment> -->
  <Applications>
    <Application Id="App" Executable="app/A.exe" EntryPoint="Windows.FullTrustApplication">
      <Extensions>
        <uap3:Extension Category="windows.appExecutionAlias"><uap3:AppExecutionAlias/></uap3:Extension>
        <uap5:Extension Category="windows.startupTask" Executable="app/B.exe">
          <uap5:StartupTask TaskId="One" Enabled="false" DisplayName="A &amp; B" />
        </uap5:Extension>
        <desktop:Extension Category='windows.startupTask'>
          <desktop:StartupTask TaskId="Two" Enabled="true"/>
        </desktop:Extension>
      </Extensions>
    </Application>
  </Applications>
</Package>"#;
        let t = startup_tasks(xml);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0], ("One".into(), false, "A & B".into(), "app\\B.exe".into()));
        assert_eq!(t[1], ("Two".into(), true, String::new(), "app\\A.exe".into()));
    }
}
