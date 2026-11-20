use std::ffi::{c_char, c_long};

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub(crate) struct CrtTm {
    pub sec: i32,
    pub min: i32,
    pub hour: i32,
    pub mday: i32,
    pub mon: i32,
    pub year: i32,
    pub wday: i32,
    pub yday: i32,
    pub isdst: i32,
}

// pele's Windows layout: c_long is 32 bits, immediately after the CRT fields.
#[repr(C)]
pub struct Tm {
    crt: CrtTm,
    gmtoff: c_long,
    zone: *const c_char,
}

unsafe extern "C" {
    fn _localtime64_s(out: *mut CrtTm, time: *const i64) -> i32;
    fn _mkgmtime64(time: *mut CrtTm) -> i64;
    fn _tzset();
}

pub(crate) fn local(time: i64) -> Option<CrtTm> {
    let mut out = CrtTm::default();
    // SAFETY: the CRT writes nine i32 fields into a live CrtTm.
    (unsafe { _localtime64_s(&mut out, &time) } == 0).then_some(out)
}

/// POSIX localtime for pele's extended tm layout on Windows.
///
/// # Safety
/// `time` must point to an i64 and `out` to a writable Tm.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn localtime_r(time: *const i64, out: *mut Tm) -> *mut Tm {
    if time.is_null() || out.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: the caller supplies a live time pointer.
    let time = unsafe { *time };
    let Some(crt) = local(time) else { return std::ptr::null_mut() };
    let mut utc = crt;
    // SAFETY: the CRT reads and updates nine initialized fields.
    let offset = unsafe { _mkgmtime64(&mut utc) } - time;
    // SAFETY: the caller supplies the full extended layout, not a CRT tm.
    unsafe { out.write(Tm { crt, gmtoff: offset as c_long, zone: std::ptr::null() }) };
    out
}

#[unsafe(no_mangle)]
pub extern "C" fn tzset() {
    // SAFETY: the CRT initializes its time-zone data.
    unsafe { _tzset() };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extended_layout_matches_pele_on_windows_x64() {
        assert_eq!(std::mem::size_of::<CrtTm>(), 36);
        assert_eq!(std::mem::offset_of!(Tm, gmtoff), 36);
        assert_eq!(std::mem::offset_of!(Tm, zone), 40);
        assert_eq!(std::mem::size_of::<Tm>(), 48);
    }

    #[test]
    fn shim_fills_the_extended_layout_without_overwriting_its_guard() {
        #[repr(C)]
        struct Guarded { tm: Tm, guard: u64 }
        let time = 1_700_000_000;
        let mut out = Guarded { tm: Tm { crt: CrtTm::default(), gmtoff: 0, zone: std::ptr::null() }, guard: 0x123456789abcdef0 };
        // SAFETY: both pointers refer to the expected live types.
        let result = unsafe { localtime_r(&time, &mut out.tm) };
        assert_eq!(result, &mut out.tm as *mut Tm);
        assert_eq!(out.tm.crt.year, 123);
        assert!((0..24).contains(&out.tm.crt.hour));
        assert!((-86400..86400).contains(&out.tm.gmtoff));
        assert_eq!(out.guard, 0x123456789abcdef0);
        tzset();
    }

    #[test]
    fn missing_or_out_of_range_time_fails_without_writing() {
        let mut out = Tm { crt: CrtTm::default(), gmtoff: 42, zone: std::ptr::null() };
        let invalid = i64::MAX;
        // SAFETY: null pointers are explicitly checked by the shim.
        unsafe {
            assert!(localtime_r(std::ptr::null(), &mut out).is_null());
            assert!(localtime_r(&invalid, std::ptr::null_mut()).is_null());
            assert!(localtime_r(&invalid, &mut out).is_null());
        }
        assert_eq!(out.gmtoff, 42);
        assert!(local(invalid).is_none());
    }
}
