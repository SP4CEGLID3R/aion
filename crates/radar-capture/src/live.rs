//! Live capture through Npcap (Windows) or libpcap (Linux/macOS).
//!
//! The library is loaded at run time, so building needs no SDK and the app
//! still starts (with a clear error) on a machine without Npcap.
//!
//! Capture is **passive and receive-only**: the adapter is opened
//! non-promiscuously with a `tcp` filter, packets are copied, nothing is ever
//! sent, injected or modified, and the game's process is never touched.

use std::ffi::{c_char, c_int, c_long, c_uint, c_void, CStr, CString};
use std::net::Ipv4Addr;
use std::ptr;
use std::sync::Arc;

use libloading::Library;
use serde::Serialize;
use thiserror::Error;

const ERRBUF: usize = 256;
const SNAPLEN: c_int = 262_144;
/// Read timeout in ms; irrelevant in non-blocking mode but required.
const TIMEOUT_MS: c_int = 50;
const NETMASK_UNKNOWN: u32 = 0xFFFF_FFFF;
const IF_LOOPBACK: c_uint = 0x1;

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error(
        "packet capture library not found ({0}). On Windows install Npcap from https://npcap.com \
         (default options), then restart the radar"
    )]
    NoLibrary(String),
    #[error("{0}")]
    Pcap(String),
    #[error("no capture device matches {0:?}")]
    NoDevice(String),
}

#[repr(C)]
struct SockAddr {
    // BSD/macOS put a length byte first; Linux and Windows a u16 family.
    family: u16,
    data: [u8; 14],
}

#[repr(C)]
struct PcapAddr {
    next: *mut PcapAddr,
    addr: *mut SockAddr,
    netmask: *mut SockAddr,
    broadaddr: *mut SockAddr,
    dstaddr: *mut SockAddr,
}

#[repr(C)]
struct PcapIf {
    next: *mut PcapIf,
    name: *mut c_char,
    description: *mut c_char,
    addresses: *mut PcapAddr,
    flags: c_uint,
}

#[repr(C)]
struct Timeval {
    tv_sec: c_long,
    tv_usec: c_long,
}

#[repr(C)]
struct PktHdr {
    ts: Timeval,
    caplen: u32,
    len: u32,
}

#[repr(C)]
struct BpfProgram {
    bf_len: c_uint,
    bf_insns: *mut c_void,
}

type FindAllDevs = unsafe extern "C" fn(*mut *mut PcapIf, *mut c_char) -> c_int;
type FreeAllDevs = unsafe extern "C" fn(*mut PcapIf);
type OpenLive =
    unsafe extern "C" fn(*const c_char, c_int, c_int, c_int, *mut c_char) -> *mut c_void;
type Compile =
    unsafe extern "C" fn(*mut c_void, *mut BpfProgram, *const c_char, c_int, u32) -> c_int;
type SetFilter = unsafe extern "C" fn(*mut c_void, *mut BpfProgram) -> c_int;
type FreeCode = unsafe extern "C" fn(*mut BpfProgram);
type SetNonblock = unsafe extern "C" fn(*mut c_void, c_int, *mut c_char) -> c_int;
type NextEx = unsafe extern "C" fn(*mut c_void, *mut *mut PktHdr, *mut *const u8) -> c_int;
type Datalink = unsafe extern "C" fn(*mut c_void) -> c_int;
type GetErr = unsafe extern "C" fn(*mut c_void) -> *mut c_char;
type Close = unsafe extern "C" fn(*mut c_void);
type LibVersion = unsafe extern "C" fn() -> *const c_char;

/// The loaded capture library.
pub struct PcapApi {
    _lib: Library,
    findalldevs: FindAllDevs,
    freealldevs: FreeAllDevs,
    open_live: OpenLive,
    compile: Compile,
    setfilter: SetFilter,
    freecode: FreeCode,
    setnonblock: SetNonblock,
    next_ex: NextEx,
    datalink: Datalink,
    geterr: GetErr,
    close: Close,
    version: Option<LibVersion>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub name: String,
    pub description: Option<String>,
    pub ipv4: Vec<String>,
    pub loopback: bool,
}

impl Device {
    /// A human-readable name: on Windows the adapter name alone is a GUID.
    pub fn label(&self) -> String {
        let mut s = self
            .description
            .clone()
            .unwrap_or_else(|| self.name.clone());
        if !self.ipv4.is_empty() {
            s += &format!(" ({})", self.ipv4.join(", "));
        }
        s
    }
}

fn candidates() -> Vec<String> {
    if cfg!(windows) {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
        vec![
            format!("{root}\\System32\\Npcap\\wpcap.dll"),
            "wpcap.dll".into(),
        ]
    } else if cfg!(target_os = "macos") {
        vec!["libpcap.A.dylib".into(), "libpcap.dylib".into()]
    } else {
        vec![
            "libpcap.so.1".into(),
            "libpcap.so.0.8".into(),
            "libpcap.so".into(),
        ]
    }
}

fn open_library(path: &str) -> Result<Library, libloading::Error> {
    #[cfg(windows)]
    {
        use libloading::os::windows::{Library as WinLibrary, LOAD_WITH_ALTERED_SEARCH_PATH};
        if std::path::Path::new(path).is_absolute() {
            // Resolve wpcap.dll's own dependency (Packet.dll) from its folder.
            return unsafe { WinLibrary::load_with_flags(path, LOAD_WITH_ALTERED_SEARCH_PATH) }
                .map(Into::into);
        }
    }
    unsafe { Library::new(path) }
}

fn cstr(p: *const c_char) -> Option<String> {
    (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

fn errbuf_text(buf: &[c_char; ERRBUF]) -> String {
    cstr(buf.as_ptr()).unwrap_or_default()
}

impl PcapApi {
    pub fn load() -> Result<Arc<PcapApi>, CaptureError> {
        let mut errors = Vec::new();
        for path in candidates() {
            match open_library(&path) {
                Ok(lib) => return Self::bind(lib).map(Arc::new),
                Err(e) => errors.push(format!("{path}: {e}")),
            }
        }
        Err(CaptureError::NoLibrary(errors.join("; ")))
    }

    fn bind(lib: Library) -> Result<PcapApi, CaptureError> {
        unsafe {
            macro_rules! sym {
                ($name:literal) => {
                    *lib.get(concat!($name, "\0").as_bytes())
                        .map_err(|e| CaptureError::Pcap(format!("{} missing: {e}", $name)))?
                };
            }
            Ok(PcapApi {
                findalldevs: sym!("pcap_findalldevs"),
                freealldevs: sym!("pcap_freealldevs"),
                open_live: sym!("pcap_open_live"),
                compile: sym!("pcap_compile"),
                setfilter: sym!("pcap_setfilter"),
                freecode: sym!("pcap_freecode"),
                setnonblock: sym!("pcap_setnonblock"),
                next_ex: sym!("pcap_next_ex"),
                datalink: sym!("pcap_datalink"),
                geterr: sym!("pcap_geterr"),
                close: sym!("pcap_close"),
                version: lib
                    .get::<LibVersion>(b"pcap_lib_version\0")
                    .ok()
                    .map(|s| *s),
                _lib: lib,
            })
        }
    }

    pub fn version(&self) -> String {
        self.version
            .and_then(|f| cstr(unsafe { f() }))
            .unwrap_or_else(|| "unknown".into())
    }

    pub fn devices(&self) -> Result<Vec<Device>, CaptureError> {
        let mut err = [0 as c_char; ERRBUF];
        let mut list: *mut PcapIf = ptr::null_mut();
        if unsafe { (self.findalldevs)(&mut list, err.as_mut_ptr()) } != 0 {
            return Err(CaptureError::Pcap(errbuf_text(&err)));
        }
        let mut out = Vec::new();
        let mut cur = list;
        while !cur.is_null() {
            let dev = unsafe { &*cur };
            let mut ipv4 = Vec::new();
            let mut a = dev.addresses;
            while !a.is_null() {
                let addr = unsafe { &*a };
                if !addr.addr.is_null() {
                    let sa = unsafe { &*addr.addr };
                    // AF_INET = 2 on Windows and Linux; on BSD the family is the second byte.
                    let family = if cfg!(any(target_os = "macos", target_os = "freebsd")) {
                        sa.family >> 8
                    } else {
                        sa.family
                    };
                    if family == 2 {
                        ipv4.push(
                            Ipv4Addr::new(sa.data[2], sa.data[3], sa.data[4], sa.data[5])
                                .to_string(),
                        );
                    }
                }
                a = addr.next;
            }
            if let Some(name) = cstr(dev.name) {
                out.push(Device {
                    name,
                    description: cstr(dev.description),
                    ipv4,
                    loopback: dev.flags & IF_LOOPBACK != 0,
                });
            }
            cur = dev.next;
        }
        unsafe { (self.freealldevs)(list) };
        Ok(out)
    }

    /// Opens a device for passive, non-blocking capture of TCP.
    pub fn open(self: &Arc<Self>, device: &str) -> Result<Handle, CaptureError> {
        let mut err = [0 as c_char; ERRBUF];
        let name = CString::new(device).map_err(|_| CaptureError::NoDevice(device.into()))?;
        // promisc = 0: only this machine's own traffic.
        let p =
            unsafe { (self.open_live)(name.as_ptr(), SNAPLEN, 0, TIMEOUT_MS, err.as_mut_ptr()) };
        if p.is_null() {
            return Err(CaptureError::Pcap(format!(
                "{device}: {}",
                errbuf_text(&err)
            )));
        }
        let handle = Handle {
            api: self.clone(),
            p,
            device: device.to_owned(),
            linktype: unsafe { (self.datalink)(p) } as u32,
        };
        let filter = CString::new("tcp").expect("no NUL");
        let mut prog = BpfProgram {
            bf_len: 0,
            bf_insns: ptr::null_mut(),
        };
        unsafe {
            if (self.compile)(p, &mut prog, filter.as_ptr(), 1, NETMASK_UNKNOWN) != 0 {
                return Err(CaptureError::Pcap(format!(
                    "{device}: filter: {}",
                    handle.last_error()
                )));
            }
            let rc = (self.setfilter)(p, &mut prog);
            (self.freecode)(&mut prog);
            if rc != 0 {
                return Err(CaptureError::Pcap(format!(
                    "{device}: filter: {}",
                    handle.last_error()
                )));
            }
            if (self.setnonblock)(p, 1, err.as_mut_ptr()) != 0 {
                return Err(CaptureError::Pcap(format!(
                    "{device}: {}",
                    errbuf_text(&err)
                )));
            }
        }
        Ok(handle)
    }
}

/// An open capture. Not `Send`: used on the thread that opened it.
pub struct Handle {
    api: Arc<PcapApi>,
    p: *mut c_void,
    pub device: String,
    pub linktype: u32,
}

pub enum Next<'a> {
    Packet {
        t_ms: f64,
        data: &'a [u8],
    },
    /// Nothing waiting right now.
    Empty,
}

impl Handle {
    fn last_error(&self) -> String {
        cstr(unsafe { (self.api.geterr)(self.p) }).unwrap_or_default()
    }

    /// The next waiting packet, without blocking. The slice is valid until
    /// the next call.
    pub fn next_packet(&mut self) -> Result<Next<'_>, CaptureError> {
        let mut hdr: *mut PktHdr = ptr::null_mut();
        let mut data: *const u8 = ptr::null();
        match unsafe { (self.api.next_ex)(self.p, &mut hdr, &mut data) } {
            1 => {
                let h = unsafe { &*hdr };
                let t_ms = h.ts.tv_sec as f64 * 1000.0 + h.ts.tv_usec as f64 / 1000.0;
                let data = unsafe { std::slice::from_raw_parts(data, h.caplen as usize) };
                Ok(Next::Packet { t_ms, data })
            }
            0 => Ok(Next::Empty),
            -2 => Err(CaptureError::Pcap(format!(
                "{}: capture ended",
                self.device
            ))),
            _ => Err(CaptureError::Pcap(format!(
                "{}: {}",
                self.device,
                self.last_error()
            ))),
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { (self.api.close)(self.p) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs where libpcap is installed; skipped (passes) where it is not.
    #[test]
    fn loads_the_library_and_lists_devices_when_available() {
        let Ok(api) = PcapApi::load() else {
            eprintln!("libpcap not installed; skipping");
            return;
        };
        assert!(!api.version().is_empty());
        // Listing may legitimately be empty without privileges; it must not crash.
        let _ = api.devices();
    }
}
