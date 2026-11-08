//! Slice helpers usable in `const fn` (S4-31b0). `copy_from_slice`, `fill`, range indexing and
//! `?` are not available in a const context on stable Rust; these helpers do the same work with
//! plain loops and `split_at`, and panic on a bad range exactly like the std calls they replace.

/// Early-return-on-error for `const fn`, where `?` is not allowed. `tri!(expr)` returns the
/// error unchanged; `tri!(expr, wrap)` returns `Err(wrap(error))` for a const-callable `wrap`
/// (a tuple variant constructor or a `const fn`).
macro_rules! tri {
    ($e:expr) => {
        match $e {
            Ok(value) => value,
            Err(error) => return Err(error),
        }
    };
    ($e:expr, $wrap:expr) => {
        match $e {
            Ok(value) => value,
            Err(error) => return Err($wrap(error)),
        }
    };
}
pub(crate) use tri;

/// `dst[at..at + src.len()].copy_from_slice(src)`.
pub(crate) const fn copy_at(dst: &mut [u8], at: usize, src: &[u8]) {
    assert!(at <= dst.len() && src.len() <= dst.len() - at);
    let mut i = 0;
    while i < src.len() {
        dst[at + i] = src[i];
        i += 1;
    }
}

/// `dst[from..to].fill(value)`.
pub(crate) const fn fill_range(dst: &mut [u8], from: usize, to: usize, value: u8) {
    assert!(from <= to && to <= dst.len());
    let mut i = from;
    while i < to {
        dst[i] = value;
        i += 1;
    }
}

/// `&buf[start..end]`.
pub(crate) const fn sub(buf: &[u8], start: usize, end: usize) -> &[u8] {
    assert!(start <= end && end <= buf.len());
    buf.split_at(end).0.split_at(start).1
}

/// `&mut buf[..end]`.
pub(crate) const fn head_mut(buf: &mut [u8], end: usize) -> &mut [u8] {
    assert!(end <= buf.len());
    buf.split_at_mut(end).0
}

/// `&mut buf[start..]`.
pub(crate) const fn tail_mut(buf: &mut [u8], start: usize) -> &mut [u8] {
    assert!(start <= buf.len());
    buf.split_at_mut(start).1
}

/// `a == b` for byte slices.
pub(crate) const fn bytes_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}
