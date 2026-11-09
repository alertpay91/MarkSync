//! Phones and cameras connected over USB (MTP), through Windows Portable Devices.
//! Windows doesn't show them as a drive letter, so normal file APIs can't see them.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;
use windows::core::{GUID, PCWSTR, PWSTR};
use windows::Win32::Devices::PortableDevices::*;
use windows::Win32::System::Com::*;

/// A folder on a phone, remembered by names (object ids change between connections).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PhoneSource {
    pub device: String,
    pub path: Vec<String>,
}

impl PhoneSource {
    pub fn display(&self) -> String {
        let mut s = self.device.clone();
        for p in &self.path {
            s.push_str(" › ");
            s.push_str(p);
        }
        s
    }
    /// Virtual path used for display and as the hash-cache key.
    pub fn virtual_root(&self) -> PathBuf {
        let mut p = PathBuf::from(format!("📱 {}", self.device));
        for c in &self.path {
            p.push(c);
        }
        p
    }
}

#[derive(Clone, Debug)]
pub struct Device {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub obj: String,
    pub name: String,
    pub is_folder: bool,
    pub size: u64,
    pub mtime: Option<SystemTime>,
}

/// Reference to a file on a phone.
#[derive(Clone, Debug)]
pub struct PhoneRef {
    pub dev: Arc<String>,
    pub obj: String,
}

struct Conn {
    content: IPortableDeviceContent,
    props: IPortableDeviceProperties,
    res: IPortableDeviceResources,
    _dev: IPortableDevice,
}
// Created with the free-threaded PortableDeviceFTM class: usable from any thread.
unsafe impl Send for Conn {}

fn conns() -> &'static Mutex<HashMap<String, Arc<Mutex<Conn>>>> {
    static C: OnceLock<Mutex<HashMap<String, Arc<Mutex<Conn>>>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn com_init() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

fn err(e: windows::core::Error) -> io::Error {
    io::Error::other(e.message())
}

unsafe fn take_pwstr(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    let s = p.to_string().unwrap_or_default();
    CoTaskMemFree(Some(p.0 as *const _));
    s
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn list_devices() -> Vec<Device> {
    com_init();
    unsafe {
        let Ok(mgr) = CoCreateInstance::<_, IPortableDeviceManager>(&PortableDeviceManager, None, CLSCTX_INPROC_SERVER)
        else {
            return Vec::new();
        };
        let _ = mgr.RefreshDeviceList();
        let mut n = 0u32;
        if mgr.GetDevices(std::ptr::null_mut(), &mut n).is_err() || n == 0 {
            return Vec::new();
        }
        let mut ids = vec![PWSTR::null(); n as usize];
        if mgr.GetDevices(ids.as_mut_ptr(), &mut n).is_err() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for p in ids.into_iter().take(n as usize) {
            let id = take_pwstr(p);
            let wid = wide(&id);
            let mut len = 0u32;
            let _ = mgr.GetDeviceFriendlyName(PCWSTR(wid.as_ptr()), PWSTR::null(), &mut len);
            let mut name = String::new();
            if len > 0 {
                let mut buf = vec![0u16; len as usize];
                if mgr.GetDeviceFriendlyName(PCWSTR(wid.as_ptr()), PWSTR(buf.as_mut_ptr()), &mut len).is_ok() {
                    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                    name = String::from_utf16_lossy(&buf[..end]);
                }
            }
            if name.is_empty() {
                name = "Phone".into();
            }
            out.push(Device { id, name });
        }
        out
    }
}

fn connect(dev_id: &str) -> io::Result<Arc<Mutex<Conn>>> {
    com_init();
    if let Some(c) = conns().lock().unwrap().get(dev_id) {
        return Ok(c.clone());
    }
    unsafe {
        let info: IPortableDeviceValues =
            CoCreateInstance(&PortableDeviceValues, None, CLSCTX_INPROC_SERVER).map_err(err)?;
        let name = wide("MarkSync");
        info.SetStringValue(&WPD_CLIENT_NAME, PCWSTR(name.as_ptr())).map_err(err)?;
        info.SetUnsignedIntegerValue(&WPD_CLIENT_MAJOR_VERSION, 1).map_err(err)?;
        info.SetUnsignedIntegerValue(&WPD_CLIENT_MINOR_VERSION, 0).map_err(err)?;
        info.SetUnsignedIntegerValue(&WPD_CLIENT_REVISION, 0).map_err(err)?;
        info.SetUnsignedIntegerValue(&WPD_CLIENT_DESIRED_ACCESS, 0x8000_0000).map_err(err)?; // GENERIC_READ
        let dev: IPortableDevice = CoCreateInstance(&PortableDeviceFTM, None, CLSCTX_INPROC_SERVER).map_err(err)?;
        let wid = wide(dev_id);
        dev.Open(PCWSTR(wid.as_ptr()), &info).map_err(err)?;
        let content = dev.Content().map_err(err)?;
        let props = content.Properties().map_err(err)?;
        let res = content.Transfer().map_err(err)?;
        let c = Arc::new(Mutex::new(Conn { content, props, res, _dev: dev }));
        conns().lock().unwrap().insert(dev_id.to_string(), c.clone());
        Ok(c)
    }
}

/// Forget open connections (after the phone was unplugged).
pub fn reset() {
    conns().lock().unwrap().clear();
}

fn ole_date_to_system(d: f64) -> Option<SystemTime> {
    use chrono::TimeZone;
    if !(d > 30000.0 && d < 80000.0) {
        return None; // unset (1899) or nonsense
    }
    let secs = ((d - 25569.0) * 86400.0).round() as i64;
    let naive = chrono::DateTime::from_timestamp(secs, 0)?.naive_utc();
    let local = chrono::Local.from_local_datetime(&naive).earliest()?;
    Some(local.into())
}

fn children_ids(c: &Conn, parent: &str) -> io::Result<Vec<String>> {
    unsafe {
        let wp = wide(parent);
        let en = c.content.EnumObjects(0, PCWSTR(wp.as_ptr()), None).map_err(err)?;
        let mut out = Vec::new();
        loop {
            let mut buf = [PWSTR::null(); 128];
            let mut got = 0u32;
            let hr = en.Next(&mut buf, &mut got);
            for p in buf.iter().take(got as usize) {
                out.push(take_pwstr(*p));
            }
            // a failure part-way must not look like a shorter, complete list
            if hr.is_err() {
                return Err(io::Error::other(hr.message()));
            }
            if got == 0 || got < buf.len() as u32 {
                break;
            }
        }
        Ok(out)
    }
}

fn keys() -> io::Result<IPortableDeviceKeyCollection> {
    unsafe {
        let k: IPortableDeviceKeyCollection =
            CoCreateInstance(&PortableDeviceKeyCollection, None, CLSCTX_INPROC_SERVER).map_err(err)?;
        for key in [
            &WPD_OBJECT_NAME,
            &WPD_OBJECT_ORIGINAL_FILE_NAME,
            &WPD_OBJECT_CONTENT_TYPE,
            &WPD_OBJECT_SIZE,
            &WPD_OBJECT_DATE_MODIFIED,
            &WPD_OBJECT_DATE_CREATED,
        ] {
            k.Add(key).map_err(err)?;
        }
        Ok(k)
    }
}

fn entry(c: &Conn, k: &IPortableDeviceKeyCollection, obj: String) -> Option<Entry> {
    unsafe {
        let v = c.props.GetValues(PCWSTR(wide(&obj).as_ptr()), k).ok()?;
        let mut name = v.GetStringValue(&WPD_OBJECT_ORIGINAL_FILE_NAME).map(|p| take_pwstr(p)).unwrap_or_default();
        if name.is_empty() {
            name = v.GetStringValue(&WPD_OBJECT_NAME).map(|p| take_pwstr(p)).unwrap_or_default();
        }
        let ct: GUID = v.GetGuidValue(&WPD_OBJECT_CONTENT_TYPE).unwrap_or_default();
        let is_folder = ct == WPD_CONTENT_TYPE_FOLDER || ct == WPD_CONTENT_TYPE_FUNCTIONAL_OBJECT;
        let size = v.GetUnsignedLargeIntegerValue(&WPD_OBJECT_SIZE).unwrap_or(0);
        let date = |key| -> Option<SystemTime> {
            let pv = v.GetValue(key).ok()?;
            ole_date_to_system(f64::try_from(&pv).ok()?)
        };
        let mtime = date(&WPD_OBJECT_DATE_MODIFIED).or_else(|| date(&WPD_OBJECT_DATE_CREATED));
        Some(Entry { obj, name, is_folder, size, mtime })
    }
}

/// Folders and files directly inside `parent` ("DEVICE" = the phone itself).
pub fn children(dev_id: &str, parent: &str) -> io::Result<Vec<Entry>> {
    let c = connect(dev_id)?;
    let c = c.lock().unwrap();
    let k = keys()?;
    let mut v: Vec<Entry> = children_ids(&c, parent)?.into_iter().filter_map(|o| entry(&c, &k, o)).collect();
    v.sort_by_key(|e| (!e.is_folder, e.name.to_lowercase()));
    Ok(v)
}

pub fn device_id_by_name(name: &str) -> Option<String> {
    list_devices().into_iter().find(|d| d.name == name).map(|d| d.id)
}

/// Finds a folder by names, e.g. ["Internal shared storage", "DCIM", "Camera"].
pub fn resolve(dev_id: &str, path: &[String]) -> io::Result<String> {
    let mut cur = WPD_DEVICE_OBJECT_ID_STR.to_string();
    for name in path {
        let kids = children(dev_id, &cur)?;
        cur = kids
            .into_iter()
            .find(|e| e.is_folder && e.name.eq_ignore_ascii_case(name))
            .map(|e| e.obj)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, name.clone()))?;
    }
    Ok(cur)
}

pub const WPD_DEVICE_OBJECT_ID_STR: &str = "DEVICE";

/// Every file below `folder`, with its path relative to it.
pub fn walk(
    dev_id: &str,
    folder: &str,
    cancel: &AtomicBool,
    found: &mut dyn FnMut(PathBuf, Entry),
) -> io::Result<usize> {
    let mut skipped = 0;
    let c = connect(dev_id)?;
    let k = keys()?;
    let mut stack: Vec<(String, PathBuf)> = vec![(folder.to_string(), PathBuf::new())];
    while let Some((obj, rel)) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let ids = { children_ids(&c.lock().unwrap(), &obj)? };
        for id in ids {
            if cancel.load(Ordering::Relaxed) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
            }
            let Some(e) = entry(&c.lock().unwrap(), &k, id) else {
                skipped += 1;
                continue;
            };
            let p = rel.join(&e.name);
            if e.is_folder {
                if !e.name.starts_with('.') {
                    stack.push((e.obj.clone(), p));
                }
            } else {
                found(p, e);
            }
        }
    }
    Ok(skipped)
}

/// Current name/size of a phone object (to check it before copying).
pub fn stat(r: &PhoneRef) -> io::Result<Entry> {
    let c = connect(&r.dev)?;
    let c = c.lock().unwrap();
    entry(&c, &keys()?, r.obj.clone()).ok_or_else(|| io::Error::other("object not found"))
}

/// One transfer at a time: MTP is a single pipe and parallel reads only fight over it.
fn io_lock() -> &'static Mutex<()> {
    static L: Mutex<()> = Mutex::new(());
    &L
}

/// Streams a file from the phone.
pub struct PhoneReader {
    stream: IStream,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Read for PhoneReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut got = 0u32;
        let want = buf.len().min(u32::MAX as usize) as u32;
        let hr = unsafe { self.stream.Read(buf.as_mut_ptr() as *mut _, want, Some(&mut got)) };
        if hr.is_err() {
            return Err(io::Error::other(hr.message()));
        }
        Ok(got as usize)
    }
}

pub fn open(r: &PhoneRef) -> io::Result<PhoneReader> {
    let guard = io_lock().lock().unwrap_or_else(|e| e.into_inner());
    let c = connect(&r.dev)?;
    let c = c.lock().unwrap();
    unsafe {
        let mut opt = 0u32;
        let mut stream: Option<IStream> = None;
        let wo = wide(&r.obj);
        c.res
            .GetStream(PCWSTR(wo.as_ptr()), &WPD_RESOURCE_DEFAULT, 0 /* STGM_READ */, &mut opt, &mut stream)
            .map_err(err)?;
        stream.map(|stream| PhoneReader { stream, _guard: guard }).ok_or_else(|| io::Error::other("no stream"))
    }
}

/// The first `n` bytes (for EXIF). The rest is still read and thrown away: Android's MTP
/// gets stuck ("The device is unreachable") after many transfers that were cut short.
pub fn read_prefix(r: &PhoneRef, n: usize) -> io::Result<Vec<u8>> {
    let mut rd = open(r)?;
    let mut buf = Vec::with_capacity(n);
    (&mut rd).take(n as u64).read_to_end(&mut buf)?;
    io::copy(&mut rd, &mut io::sink())?;
    Ok(buf)
}
