use crate::local::{Access, BoxSpec};
use crate::util::*;
use crate::{deny, ensure};
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Security::Authorization::*;
use windows_sys::Win32::Security::Isolation::*;
use windows_sys::Win32::Security::*;
use windows_sys::Win32::Storage::FileSystem::{DELETE, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE};
use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
use windows_sys::Win32::System::JobObjects::*;
use windows_sys::Win32::System::SystemServices::{ACCESS_ALLOWED_ACE_TYPE, ACCESS_ALLOWED_CALLBACK_ACE_TYPE, ACCESS_DENIED_ACE_TYPE, ACCESS_DENIED_CALLBACK_ACE_TYPE, SE_GROUP_ENABLED};
use windows_sys::Win32::System::Threading::*;

const PROFILE: &str = "kern-box";

static LAUNCH: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn wide(s: &str) -> Vec<u16> { OsStr::new(s).encode_wide().chain([0]).collect() }

struct LocalPtr(*mut core::ffi::c_void);
impl Drop for LocalPtr {
    fn drop(&mut self) { if !self.0.is_null() { unsafe { LocalFree(self.0 as HLOCAL); } } }
}

struct Profile { name: Vec<u16>, sid: Sid }
impl Profile {
    fn create(name: &str) -> R<Profile> {
        let name = wide(name);
        let mut raw: PSID = null_mut();
        unsafe {
            let hr = CreateAppContainerProfile(name.as_ptr(), name.as_ptr(), name.as_ptr(), null(), 0, &mut raw);
            ensure!(hr >= 0, "Could not create the Kern box identity (0x{:08x})", hr as u32);
            let sid = copy_sid(raw);
            FreeSid(raw);
            Ok(Profile { name, sid: sid? })
        }
    }
}
impl Drop for Profile {
    fn drop(&mut self) { unsafe { DeleteAppContainerProfile(self.name.as_ptr()); } }
}
fn alive(pid: u32) -> bool {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() { return GetLastError() == ERROR_ACCESS_DENIED; }
        let process = Owned(h);
        let mut code = 0u32;
        GetExitCodeProcess(process.0, &mut code) == 0 || code == STILL_ACTIVE as u32
    }
}
fn sweep() {
    let Ok(local) = std::env::var("LOCALAPPDATA") else { return };
    for e in std::fs::read_dir(std::path::Path::new(&local).join("Packages")).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let pid = name.strip_prefix(&format!("{PROFILE}-")).and_then(|r| r.split_once('-')).and_then(|(p, _)| p.parse::<u32>().ok());
        if pid.is_some_and(|p| !alive(p)) { unsafe { DeleteAppContainerProfile(wide(&name).as_ptr()); } }
    }
}

struct Sid(Vec<u8>);
impl Sid {
    fn well_known(kind: WELL_KNOWN_SID_TYPE) -> R<Sid> {
        let mut len = 0u32;
        unsafe {
            CreateWellKnownSid(kind, null_mut(), null_mut(), &mut len);
            let mut buf = vec![0u8; len as usize];
            ensure!(CreateWellKnownSid(kind, null_mut(), buf.as_mut_ptr() as PSID, &mut len) != 0, "Capability SID unavailable");
            Ok(Sid(buf))
        }
    }
    fn ptr(&self) -> PSID { self.0.as_ptr() as PSID }
}
unsafe fn copy_sid(raw: PSID) -> R<Sid> {
    unsafe {
        let len = GetLengthSid(raw);
        let mut buf = vec![0u8; len as usize];
        ensure!(CopySid(len, buf.as_mut_ptr() as PSID, raw) != 0, "Box identity copy failed");
        Ok(Sid(buf))
    }
}

fn apply(path: &std::path::Path, sid: &Sid, mode: ACCESS_MODE, mask: u32, inherit: bool) -> R<()> {
    let wpath = wide(&path.to_string_lossy());
    let mut dacl: *mut ACL = null_mut();
    let mut sd: PSECURITY_DESCRIPTOR = null_mut();
    unsafe {
        let got = GetNamedSecurityInfoW(wpath.as_ptr(), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION, null_mut(), null_mut(), &mut dacl, null_mut(), &mut sd);
        ensure!(got == 0, "Cannot read permissions of {} ({got})", path.display());
        let _owned = LocalPtr(sd);
        let ea = EXPLICIT_ACCESS_W {
            grfAccessPermissions: mask,
            grfAccessMode: mode,
            grfInheritance: if inherit { SUB_CONTAINERS_AND_OBJECTS_INHERIT } else { NO_INHERITANCE },
            Trustee: TRUSTEE_W { pMultipleTrustee: null_mut(), MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE, TrusteeForm: TRUSTEE_IS_SID, TrusteeType: TRUSTEE_IS_WELL_KNOWN_GROUP, ptstrName: sid.ptr() as *mut u16 },
        };
        let mut merged: *mut ACL = null_mut();
        let set = SetEntriesInAclW(1, &ea, dacl, &mut merged);
        ensure!(set == 0, "Cannot compose permissions for {} ({set})", path.display());
        let _owned_acl = LocalPtr(merged as *mut core::ffi::c_void);
        let wrote = SetNamedSecurityInfoW(wpath.as_ptr() as *mut u16, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION, null_mut(), null_mut(), merged, null_mut());
        ensure!(wrote == 0, "Cannot set permissions on {} ({wrote})", path.display());
    }
    Ok(())
}
fn revoke(path: &std::path::Path, sid: &Sid) { let _ = apply(path, sid, REVOKE_ACCESS, 0, false); }

fn app_containers_can_read(path: &std::path::Path) -> R<bool> {
    let wpath = wide(&path.to_string_lossy());
    let mut dacl: *mut ACL = null_mut();
    let mut sd: PSECURITY_DESCRIPTOR = null_mut();
    unsafe {
        let got = GetNamedSecurityInfoW(wpath.as_ptr(), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION, null_mut(), null_mut(), &mut dacl, null_mut(), &mut sd);
        let _owned = LocalPtr(sd);
        ensure!(got == 0, "Cannot read permissions of denied path {} ({got})", path.display());
        if dacl.is_null() { return Ok(true); }
        for i in 0..(*dacl).AceCount as u32 {
            let mut ace: *mut core::ffi::c_void = null_mut();
            ensure!(GetAce(dacl, i, &mut ace) != 0, "Cannot read permissions of denied path {} ({})", path.display(), GetLastError());
            let b = std::slice::from_raw_parts(ace as *const u8, (*(ace as *const ACE_HEADER)).AceSize as usize);
            match b[0] as u32 {
                ACCESS_DENIED_ACE_TYPE | ACCESS_DENIED_CALLBACK_ACE_TYPE => {}
                ACCESS_ALLOWED_ACE_TYPE | ACCESS_ALLOWED_CALLBACK_ACE_TYPE => {
                    let sub = |n: usize| b.get(16 + 4 * n..20 + 4 * n).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]));
                    let every_package = b.get(9) == Some(&2) && sub(0) == Some(2) && matches!(sub(1), Some(1 | 2));
                    if b.get(10..16) == Some(&[0, 0, 0, 0, 0, 15][..]) && (every_package || sub(0) == Some(3)) { return Ok(true); }
                }
                _ => deny!("Denied path {} has a permission entry Kern cannot evaluate", path.display()),
            }
        }
    }
    Ok(false)
}
fn unreachable(path: &std::path::Path, granted: &[&std::path::Path], budget: &mut u32) -> R<()> {
    if granted.contains(&path) { return Ok(()); }
    *budget = budget.checked_sub(1).ok_or_else(|| Denied(format!("Denied path {} is too large to verify; deny a narrower path", path.display())))?;
    let meta = std::fs::symlink_metadata(path)?;
    ensure!(!meta.file_type().is_symlink(), "Denied path {} contains a link Kern cannot verify", path.display());
    ensure!(!app_containers_can_read(path)?, "Every app container can read denied path {}; Kern cannot deny it without changing its permissions", path.display());
    if meta.is_dir() { for e in std::fs::read_dir(path)? { unreachable(&e?.path(), granted, budget)?; } }
    Ok(())
}

fn capabilities(net: bool) -> R<(Vec<Sid>, Vec<SID_AND_ATTRIBUTES>)> {
    let sids = if net {
        vec![Sid::well_known(WinCapabilityInternetClientSid)?, Sid::well_known(WinCapabilityInternetClientServerSid)?, Sid::well_known(WinCapabilityPrivateNetworkClientServerSid)?]
    } else {
        Vec::new()
    };
    let attrs = sids.iter().map(|s| SID_AND_ATTRIBUTES { Sid: s.ptr(), Attributes: SE_GROUP_ENABLED as u32 }).collect();
    Ok((sids, attrs))
}

struct Owned(HANDLE);
impl Drop for Owned {
    fn drop(&mut self) { if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE { unsafe { CloseHandle(self.0); } } }
}

pub fn run_box(spec: &BoxSpec) -> R<u32> {
    ensure!(!spec.argv.is_empty(), "The box needs a command to run");
    let _serialized = LAUNCH.lock().unwrap_or_else(|e| e.into_inner());
    sweep();
    let profile = Profile::create(&format!("{PROFILE}-{}-{}", std::process::id(), uuid4()?))?;
    let sid = &profile.sid;
    let mut applied: Vec<&std::path::Path> = Vec::new();
    let result = (|| -> R<u32> {
        let granted: Vec<&std::path::Path> = spec.grants.iter().filter(|(_, a)| *a != Access::Deny).map(|(p, _)| p.as_path()).collect();
        for (path, access) in spec.grants {
            let mask = match access {
                Access::ReadWrite => FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE | DELETE,
                Access::ReadExec => FILE_GENERIC_READ | FILE_GENERIC_EXECUTE,
                Access::Deny => { unreachable(path, &granted, &mut 20_000)?; continue; }
            };
            apply(path, sid, GRANT_ACCESS, mask, true)?;
            applied.push(path);
        }
        let (_cap_sids, cap_attrs) = capabilities(spec.net)?;
        let mut sc = SECURITY_CAPABILITIES {
            AppContainerSid: sid.ptr(),
            Capabilities: if cap_attrs.is_empty() { null_mut() } else { cap_attrs.as_ptr() as *mut SID_AND_ATTRIBUTES },
            CapabilityCount: cap_attrs.len() as u32,
            Reserved: 0,
        };
        unsafe { launch(spec, &mut sc) }
    })();
    for path in applied.iter().rev() { revoke(path, sid); }
    result
}

unsafe fn launch(spec: &BoxSpec, sc: &mut SECURITY_CAPABILITIES) -> R<u32> {
    unsafe {
        let mut size = 0usize;
        InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut size);
        let mut attr = vec![0u8; size];
        let list = attr.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
        ensure!(InitializeProcThreadAttributeList(list, 1, 0, &mut size) != 0, "Box attribute list init failed ({})", GetLastError());
        let ok = UpdateProcThreadAttribute(list, 0, PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize, sc as *mut _ as *const core::ffi::c_void, size_of::<SECURITY_CAPABILITIES>(), null_mut(), null_mut());
        ensure!(ok != 0, "Box capability attribute failed ({})", GetLastError());

        let mut si: STARTUPINFOEXW = std::mem::zeroed();
        si.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        si.lpAttributeList = list;
        let (hin, hout, herr) = (GetStdHandle(STD_INPUT_HANDLE), GetStdHandle(STD_OUTPUT_HANDLE), GetStdHandle(STD_ERROR_HANDLE));
        for h in [hin, hout, herr] {
            if !h.is_null() && h != INVALID_HANDLE_VALUE { SetHandleInformation(h, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT); }
        }
        si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        si.StartupInfo.hStdInput = hin;
        si.StartupInfo.hStdOutput = hout;
        si.StartupInfo.hStdError = herr;

        let mut cmdline = wide(&command_line(spec.argv));
        let cwd = wide(&spec.cwd.to_string_lossy());
        let env_block = env_block(spec.env);

        let job = CreateJobObjectW(null_mut(), null_mut());
        ensure!(!job.is_null(), "Box job object unavailable ({})", GetLastError());
        let job = Owned(job);
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(job.0, JobObjectExtendedLimitInformation, &info as *const _ as *const core::ffi::c_void, size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32);

        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
        let created = CreateProcessW(
            null(), cmdline.as_mut_ptr(), null_mut(), null_mut(), 1,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT,
            env_block.as_ptr() as *const core::ffi::c_void, cwd.as_ptr(), &si.StartupInfo, &mut pi,
        );
        let err = GetLastError();
        DeleteProcThreadAttributeList(list);
        ensure!(created != 0, "The box could not start the command (CreateProcess error {err}). Ensure the tool directory is granted and the executable path is correct.");
        let process = Owned(pi.hProcess);
        let thread = Owned(pi.hThread);
        ensure!(AssignProcessToJobObject(job.0, process.0) != 0, "Box job assignment failed ({})", GetLastError());
        ResumeThread(thread.0);
        if WaitForSingleObject(process.0, spec.timeout_ms) == WAIT_TIMEOUT {
            TerminateJobObject(job.0, 1);
            WaitForSingleObject(process.0, 5000);
            deny!("The box timed out after {} seconds and every process in it was terminated", spec.timeout_ms / 1000);
        }
        let mut code = 0u32;
        ensure!(GetExitCodeProcess(process.0, &mut code) != 0, "Box exit code unavailable ({})", GetLastError());
        Ok(code)
    }
}

fn command_line(argv: &[String]) -> String {
    argv.iter().map(|a| quote_arg(a)).collect::<Vec<_>>().join(" ")
}
fn quote_arg(a: &str) -> String {
    if !a.is_empty() && !a.bytes().any(|b| matches!(b, b' ' | b'\t' | b'\n' | 0x0b | b'"')) {
        return a.to_string();
    }
    let mut s = String::from("\"");
    let mut backslashes = 0usize;
    for c in a.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => { s.push_str(&"\\".repeat(backslashes * 2 + 1)); s.push('"'); backslashes = 0; }
            _ => { s.push_str(&"\\".repeat(backslashes)); s.push(c); backslashes = 0; }
        }
    }
    s.push_str(&"\\".repeat(backslashes * 2));
    s.push('"');
    s
}
fn env_block(env: &[(String, String)]) -> Vec<u16> {
    let mut entries: Vec<&(String, String)> = env.iter().collect();
    entries.sort_by_key(|e| e.0.to_ascii_uppercase());
    let mut out = Vec::new();
    for (k, v) in entries {
        out.extend(wide(&format!("{k}={v}")));
    }
    if out.is_empty() { out.push(0); }
    out.push(0);
    out
}
