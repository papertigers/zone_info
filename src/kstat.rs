use thiserror::Error;

type Result<T> = std::result::Result<T, KstatError>;

#[derive(Error, Debug)]
pub enum KstatError {
    #[error("kstat_open(3KSTAT) failed")]
    Open(std::io::Error),
    #[error("GETZONEID(3C) failed")]
    Zoneid(std::io::Error),
    #[error("`{0}`")]
    Error(String),
}

/*
 * This is a copy paste from https://github.com/FillZpp/sys-info-rs/blob/master/kstat.rs
 */

// There is presently no static CStr constructor, so use these constants with
// the c() wrapper below:
const MODULE_CAPS: &[u8] = b"caps\0";
const MODULE_UNIX: &[u8] = b"unix\0";

const NAME_SYSTEM_MISC: &[u8] = b"system_misc\0";

const STAT_VALUE: &[u8] = b"value\0";
const STAT_NCPUS: &[u8] = b"ncpus\0";

fn c(buf: &[u8]) -> &std::ffi::CStr {
    std::ffi::CStr::from_bytes_with_nul(buf).expect("invalid string constant")
}

mod wrapper {
    use std::ffi::CStr;
    use std::os::raw::c_char;
    use std::os::raw::c_int;
    use std::os::raw::c_long;
    use std::os::raw::c_longlong;
    use std::os::raw::c_uchar;
    use std::os::raw::c_uint;
    use std::os::raw::c_ulong;
    use std::os::raw::c_void;
    use std::ptr::{null, null_mut, NonNull};

    const KSTAT_TYPE_NAMED: c_uchar = 1;

    const KSTAT_STRLEN: usize = 31;

    #[repr(C)]
    struct Kstat {
        ks_crtime: c_longlong,
        ks_next: *mut Kstat,
        ks_kid: c_uint,
        ks_module: [c_char; KSTAT_STRLEN],
        ks_resv: c_uchar,
        ks_instance: c_int,
        ks_name: [c_char; KSTAT_STRLEN],
        ks_type: c_uchar,
        ks_class: [c_char; KSTAT_STRLEN],
        ks_flags: c_uchar,
        ks_data: *mut c_void,
        ks_ndata: c_uint,
        ks_data_size: usize,
        ks_snaptime: c_longlong,
    }

    impl Kstat {
        fn name(&self) -> &CStr {
            unsafe { CStr::from_ptr(self.ks_name.as_ptr()) }
        }

        fn module(&self) -> &CStr {
            unsafe { CStr::from_ptr(self.ks_module.as_ptr()) }
        }
    }

    #[repr(C)]
    struct KstatCtl {
        kc_chain_id: c_uint,
        kc_chain: *mut Kstat,
        kc_kd: c_int,
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    union KstatValue {
        c: [c_char; 16],
        l: c_long,
        ul: c_ulong,
        ui32: u32,
    }

    #[repr(C)]
    struct KstatNamed {
        name: [c_char; KSTAT_STRLEN],
        data_type: c_uchar,
        value: KstatValue,
    }

    #[link(name = "kstat")]
    extern "C" {
        fn kstat_open() -> *mut KstatCtl;
        fn kstat_close(kc: *mut KstatCtl) -> c_int;
        fn kstat_lookup(
            kc: *mut KstatCtl,
            module: *const c_char,
            instance: c_int,
            name: *const c_char,
        ) -> *mut Kstat;
        fn kstat_read(kc: *mut KstatCtl, ksp: *mut Kstat, buf: *mut c_void) -> c_int;
        fn kstat_data_lookup(ksp: *mut Kstat, name: *const c_char) -> *mut c_void;
    }

    /// Minimal wrapper around libkstat(3LIB) on illumos and Solaris systems.
    pub struct KstatWrapper {
        kc: NonNull<KstatCtl>,
        ks: Option<NonNull<Kstat>>,
        stepping: bool,
    }

    /// Turn an optional CStr into a (const char *) for Some, or NULL for None.
    fn cp(p: &Option<&CStr>) -> *const c_char {
        p.map_or_else(null, |p| p.as_ptr())
    }

    impl KstatWrapper {
        pub fn open() -> super::Result<Self> {
            let kc = NonNull::new(unsafe { kstat_open() });
            if let Some(kc) = kc {
                Ok(KstatWrapper {
                    kc,
                    ks: None,
                    stepping: false,
                })
            } else {
                Err(super::KstatError::Open(std::io::Error::last_os_error()))
            }
        }

        /// Call kstat_lookup(3KSTAT) and store the result, if there is a match.
        pub fn lookup(&mut self, module: Option<&CStr>, name: Option<&CStr>) {
            self.ks =
                NonNull::new(unsafe { kstat_lookup(self.kc.as_ptr(), cp(&module), -1, cp(&name)) });

            self.stepping = false;
        }

        /// Call once to start iterating, and then repeatedly for each
        /// additional kstat in the chain.  Returns false once there are no more
        /// kstat entries.
        pub fn step(&mut self) -> bool {
            if !self.stepping {
                self.stepping = true;
            } else {
                self.ks = self
                    .ks
                    .and_then(|ks| NonNull::new(unsafe { ks.as_ref() }.ks_next));
            }

            if self.ks.is_none() {
                self.stepping = false;
                false
            } else {
                true
            }
        }

        /// Return the module name of the current kstat.  This routine will
        /// panic if step() has not returned true.
        pub fn module(&self) -> &CStr {
            let ks = self.ks.as_ref().expect("step() must return true first");
            unsafe { ks.as_ref() }.module()
        }

        /// Return the name of the current kstat.  This routine will panic if
        /// step() has not returned true.
        pub fn name(&self) -> &CStr {
            let ks = self.ks.as_ref().expect("step() must return true first");
            unsafe { ks.as_ref() }.name()
        }

        /// Look up a named kstat value.  For internal use by typed accessors.
        fn data_value(&self, statistic: &CStr) -> Option<NonNull<KstatNamed>> {
            let (ks, ksp) = if let Some(ks) = &self.ks {
                (unsafe { ks.as_ref() }, ks.as_ptr())
            } else {
                return None;
            };

            if unsafe { kstat_read(self.kc.as_ptr(), ksp, null_mut()) } == -1 {
                return None;
            }

            if ks.ks_type != KSTAT_TYPE_NAMED || ks.ks_ndata < 1 {
                // This is not a named kstat, or it has no data payload.
                return None;
            }

            NonNull::new(unsafe { kstat_data_lookup(ksp, cp(&Some(statistic))) })
                .map(|voidp| voidp.cast())
        }

        /// Look up a named kstat value and interpret it as a "uint32_t".
        pub fn data_u32(&self, statistic: &CStr) -> Option<u64> {
            self.data_value(statistic)
                .map(|kn| unsafe { kn.as_ref().value.ui32 } as u64)
        }

        /// Look up a named kstat value and interpret it as a "ulong_t".
        pub fn data_ulong(&self, statistic: &CStr) -> Option<u64> {
            self.data_value(statistic)
                .map(|kn| unsafe { kn.as_ref().value.ul } as u64)
        }
    }

    impl Drop for KstatWrapper {
        fn drop(&mut self) {
            unsafe { kstat_close(self.kc.as_ptr()) };
        }
    }
}

pub(crate) fn ncpus() -> Result<usize> {
    let mut k = wrapper::KstatWrapper::open()?;

    k.lookup(Some(c(MODULE_UNIX)), Some(c(NAME_SYSTEM_MISC)));
    while k.step() {
        if k.module() != c(MODULE_UNIX) || k.name() != c(NAME_SYSTEM_MISC) {
            continue;
        }

        if let Some(ncpus) = k.data_u32(c(STAT_NCPUS)) {
            return Ok(ncpus as usize);
        }
    }

    Err(KstatError::Error("cpu count kstat not found".into()))
}

pub(crate) fn zone_cpu_cap() -> Result<Option<u64>> {
    let mut k = wrapper::KstatWrapper::open()?;
    let zoneid = zonename::getzoneid().map_err(KstatError::Zoneid)?;
    let name = std::ffi::CString::new(format!("cpucaps_zone_{}", zoneid)).expect("invalid CString");

    k.lookup(Some(c(MODULE_CAPS)), Some(&name));
    while k.step() {
        if k.module() != c(MODULE_CAPS) || k.name() != name.as_c_str() {
            continue;
        }

        if let Some(value) = k.data_ulong(c(STAT_VALUE)) {
            return Ok(Some(value));
        }
    }

    Ok(None)
}
