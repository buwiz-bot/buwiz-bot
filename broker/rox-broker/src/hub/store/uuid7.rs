//! Permanent ids (state.mjs `uuidv7`): UUID v7 (RFC 9562), a 48-bit unix-ms
//! timestamp, version 7, variant 10, the rest random. rand_a (12 bits)
//! counts up within one millisecond so ids minted back to back stay
//! monotonic, one counter for the whole process as Node keeps it.

use std::sync::Mutex;

/// (the last millisecond used, its counter)
static LAST: Mutex<(i64, u16)> = Mutex::new((-1, 0));

/// Milliseconds since the epoch (`Date.now()`).
pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// `uuidv7()`: an id for now.
pub fn uuidv7() -> String {
    uuidv7_at(now_ms())
}

/// `uuidv7(ms)`: an id carrying `ms` (the backfill gives old rows their time).
pub fn uuidv7_at(ms: i64) -> String {
    let mut ms = ms.max(0);
    let mut b: [u8; 16] = rox_v1::keys::random::<16>();
    let mut ctr: u16 = (((b[6] & 0x0f) as u16) << 8) | b[7] as u16;
    {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if ms > last.0 {
            ctr &= 0x7ff; // leave headroom to count up
            *last = (ms, ctr);
        } else if ms == last.0 {
            if last.1 >= 0xfff {
                ms += 1;
                *last = (ms, 0);
            } else {
                last.1 += 1;
            }
            ctr = last.1;
        }
    }
    for (i, byte) in b.iter_mut().take(6).enumerate() {
        *byte = (ms >> (8 * (5 - i))) as u8;
    }
    b[6] = 0x70 | (ctr >> 8) as u8;
    b[7] = (ctr & 0xff) as u8;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

/// state.mjs `v7Time`: the time a v7 id carries, else `None`.
pub fn v7_time(id: Option<&str>) -> Option<i64> {
    let id = id?;
    let b = id.as_bytes();
    let hex = |r: std::ops::Range<usize>| b.get(r).is_some_and(|s| s.iter().all(u8::is_ascii_hexdigit));
    let ok = hex(0..8) && b.get(8) == Some(&b'-') && hex(9..13) && b.get(13) == Some(&b'-') && b.get(14) == Some(&b'7');
    if !ok {
        return None;
    }
    let digits: String = id.chars().filter(|c| *c != '-').take(12).collect();
    i64::from_str_radix(&digits, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_uuidv7_carries_its_time_and_counts_up_within_a_millisecond() {
        // a time ahead of every id minted so far: within it the counter counts up
        let k = now_ms() + 1_000 * 86_400_000;
        let a = uuidv7_at(k);
        let b = uuidv7_at(k);
        assert_eq!(v7_time(Some(&a)), Some(k));
        assert!(b > a, "{a} {b}");
        assert_eq!(a.len(), 36);
        assert_eq!(&a[14..15], "7");
        assert!(matches!(&a[19..20], "8" | "9" | "a" | "b"));
        assert_eq!(v7_time(Some("t1")), None);
        assert_eq!(v7_time(Some("01234567-89ab-4def-8123-456789abcdef")), None);
    }
}
