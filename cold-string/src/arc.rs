use alloc::{
    borrow::{Cow, ToOwned},
    boxed::Box,
    str::Utf8Error,
    string::String,
};
use core::{
    cmp::Ordering,
    fmt,
    hash::{Hash, Hasher},
    iter::FromIterator,
    ops::Deref,
    str,
};

#[cfg(not(all(loom, test)))]
use core::sync::atomic::{fence, AtomicUsize, Ordering as AtomicOrdering};
#[cfg(all(loom, test))]
use loom::sync::atomic::{fence, AtomicUsize, Ordering as AtomicOrdering};

use crate::encoded::Encoded;

const IMMORTAL: usize = 1;
const REF_ONE: usize = 2;
const MAX_REFCOUNT: usize = usize::MAX / 4;

/// A one-word, atomically reference-counted immutable UTF-8 string.
///
/// Strings up to one machine word are stored inline. Longer strings use one
/// allocation containing the reference count, variable-length length, and bytes.
///
/// ```
/// use cold_string::ArcColdString;
///
/// let first = ArcColdString::new("a string longer than one machine word");
/// let second = first.clone();
/// assert_eq!(first, second);
/// ```
#[repr(transparent)]
pub struct ArcColdString {
    encoded: Encoded<AtomicUsize>,
}

impl ArcColdString {
    pub fn from_utf8<B: AsRef<[u8]>>(bytes: B) -> Result<Self, Utf8Error> {
        Ok(Self::new(str::from_utf8(bytes.as_ref())?))
    }

    /// # Safety
    ///
    /// `bytes` must contain valid UTF-8.
    pub unsafe fn from_utf8_unchecked<B: AsRef<[u8]>>(bytes: B) -> Self {
        Self::new(str::from_utf8_unchecked(bytes.as_ref()))
    }

    pub fn new<T: AsRef<str>>(value: T) -> Self {
        let s = value.as_ref();
        Self {
            encoded: Encoded::new(s, AtomicUsize::new(REF_ONE)),
        }
    }

    #[rustversion::since(1.61)]
    #[inline]
    pub const fn new_inline_const(s: &str) -> Self {
        Self {
            encoded: Encoded::new_inline_const(s),
        }
    }

    #[inline]
    fn count(&self) -> &AtomicUsize {
        debug_assert!(!self.is_inline());
        unsafe { &(*self.encoded.heap_ptr().as_ptr()).header }
    }

    #[inline]
    pub fn is_inline(&self) -> bool {
        self.encoded.is_inline()
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.encoded.len()
    }

    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        self.encoded.as_bytes()
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        // SAFETY: constructors accept only valid UTF-8.
        unsafe { str::from_utf8_unchecked(self.as_bytes()) }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Clone for ArcColdString {
    #[inline]
    fn clone(&self) -> Self {
        if !self.is_inline() {
            let count = self.count();
            if count.load(AtomicOrdering::Relaxed) & IMMORTAL == 0 {
                let old = count.fetch_add(REF_ONE, AtomicOrdering::Relaxed);
                if old >> 1 >= MAX_REFCOUNT {
                    // Permanently leak the allocation instead of allowing the
                    // count to wrap. The spare range covers racing increments.
                    count.fetch_or(IMMORTAL, AtomicOrdering::Relaxed);
                }
            }
        }

        Self {
            encoded: self.encoded,
        }
    }
}

impl Drop for ArcColdString {
    #[inline]
    fn drop(&mut self) {
        if self.is_inline() {
            return;
        }

        let count = self.count();
        if count.load(AtomicOrdering::Relaxed) & IMMORTAL != 0 {
            return;
        }

        let old = count.fetch_sub(REF_ONE, AtomicOrdering::Release);
        if old & IMMORTAL != 0 || old != REF_ONE {
            return;
        }

        fence(AtomicOrdering::Acquire);
        // SAFETY: this was the last reference and the count is synchronized.
        unsafe { self.encoded.deallocate() }
    }
}

impl Default for ArcColdString {
    fn default() -> Self {
        Self::new("")
    }
}

impl Deref for ArcColdString {
    type Target = str;

    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl PartialEq for ArcColdString {
    fn eq(&self, other: &Self) -> bool {
        self.encoded.addr() == other.encoded.addr() || self.as_bytes() == other.as_bytes()
    }
}

impl Eq for ArcColdString {}

impl Hash for ArcColdString {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state)
    }
}

impl fmt::Debug for ArcColdString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl fmt::Display for ArcColdString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.as_str(), f)
    }
}

impl From<&str> for ArcColdString {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<String> for ArcColdString {
    fn from(s: String) -> Self {
        Self::new(&s)
    }
}

impl From<Box<str>> for ArcColdString {
    fn from(s: Box<str>) -> Self {
        Self::new(&s)
    }
}

impl From<ArcColdString> for String {
    fn from(s: ArcColdString) -> Self {
        s.as_str().to_owned()
    }
}

impl From<ArcColdString> for Cow<'_, str> {
    fn from(s: ArcColdString) -> Self {
        Self::Owned(s.into())
    }
}

impl<'a> From<&'a ArcColdString> for Cow<'a, str> {
    fn from(s: &'a ArcColdString) -> Self {
        Self::Borrowed(s)
    }
}

impl<'a> From<Cow<'a, str>> for ArcColdString {
    fn from(s: Cow<'a, str>) -> Self {
        Self::new(s)
    }
}

impl FromIterator<char> for ArcColdString {
    fn from_iter<I: IntoIterator<Item = char>>(iter: I) -> Self {
        Self::new(iter.into_iter().collect::<String>())
    }
}

impl core::borrow::Borrow<str> for ArcColdString {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl PartialEq<str> for ArcColdString {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<ArcColdString> for str {
    fn eq(&self, other: &ArcColdString) -> bool {
        other == self
    }
}

impl PartialEq<&str> for ArcColdString {
    fn eq(&self, other: &&str) -> bool {
        self == *other
    }
}

impl PartialEq<ArcColdString> for &str {
    fn eq(&self, other: &ArcColdString) -> bool {
        other == *self
    }
}

impl AsRef<str> for ArcColdString {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<[u8]> for ArcColdString {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl Ord for ArcColdString {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl PartialOrd for ArcColdString {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl str::FromStr for ArcColdString {
    type Err = core::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::new(s))
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for ArcColdString {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for ArcColdString {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(Self::new(s))
    }
}

unsafe impl Send for ArcColdString {}
unsafe impl Sync for ArcColdString {}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{align_of, size_of};

    type ArcInner = crate::heap::VintStringInner<AtomicUsize>;

    #[test]
    fn layout() {
        assert_eq!(size_of::<ArcColdString>(), size_of::<usize>());
        assert_eq!(
            size_of::<Option<ArcColdString>>(),
            size_of::<ArcColdString>()
        );
        assert_eq!(size_of::<ArcInner>(), size_of::<AtomicUsize>());
        assert_eq!(align_of::<ArcInner>(), align_of::<AtomicUsize>());
    }

    #[test]
    fn inline_and_heap_clone() {
        let inline = ArcColdString::new("short");
        let inline_clone = inline.clone();
        assert!(inline.is_inline());
        assert_eq!(inline, inline_clone);

        let heap = ArcColdString::new("a string longer than one machine word");
        let heap_clone = heap.clone();
        assert!(!heap.is_inline());
        assert_eq!(heap.encoded.addr(), heap_clone.encoded.addr());
        assert_eq!(heap.count().load(AtomicOrdering::Relaxed), 2 * REF_ONE);
        drop(heap_clone);
        assert_eq!(heap.count().load(AtomicOrdering::Relaxed), REF_ONE);
    }

    #[test]
    fn clones_across_threads() {
        let value = ArcColdString::new("a shared string longer than one machine word");
        let threads: alloc::vec::Vec<_> = (0..8)
            .map(|_| {
                let clone = value.clone();
                std::thread::spawn(move || {
                    assert_eq!(clone, "a shared string longer than one machine word")
                })
            })
            .collect();

        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(value.count().load(AtomicOrdering::Relaxed), REF_ONE);
    }

    #[cfg(loom)]
    #[test]
    fn loom_clone_drop() {
        loom::model(|| {
            const TEXT: &str = "a shared string longer than one machine word";

            let value = ArcColdString::new(TEXT);
            let left = value.clone();
            let right = value.clone();

            let left = loom::thread::spawn(move || {
                let clone = left.clone();
                assert_eq!(clone.as_str(), TEXT);
                drop(clone);
                drop(left);
            });
            let right = loom::thread::spawn(move || {
                let clone = right.clone();
                assert_eq!(clone.as_str(), TEXT);
                drop(right);
                drop(clone);
            });

            left.join().unwrap();
            right.join().unwrap();
            assert_eq!(value.count().load(AtomicOrdering::Relaxed), REF_ONE);
        });
    }

    #[test]
    fn const_inline_matches_runtime() {
        const VALUE: ArcColdString = ArcColdString::new_inline_const("cold");
        assert_eq!(VALUE, ArcColdString::new("cold"));
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_roundtrip_shape() {
        use serde_test::{assert_tokens, Token};

        let value = ArcColdString::new("a shared string longer than one machine word");
        assert_tokens(
            &value,
            &[Token::Str("a shared string longer than one machine word")],
        );
    }
}
